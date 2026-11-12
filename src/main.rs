mod config;
mod github;
mod install;
mod select;
mod service;
mod state;

use anyhow::{bail, Context, Result};
use config::{Config, Target};
use github::{Github, Release};
use state::{Applied, State};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 起動が保留中のターゲットを、この間隔で再試行する (許可ダイアログを出し続けないよう長めに)。
const PENDING_RETRY: Duration = Duration::from_secs(300);

const USAGE: &str = "使い方: gh-updater --config <設定.toml> [--once | --check]
  (引数なし)  常駐して interval ごとに確認・更新
  --once      1回だけ確認・更新して終了
  --check     更新が必要かを表示するだけ (何も変更しない)";

fn log(msg: impl AsRef<str>) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    eprintln!("[{secs}] {}", msg.as_ref());
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Daemon,
    Once,
    Check,
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("エラー: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let mut config_path: Option<PathBuf> = None;
    let mut mode = Mode::Daemon;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" | "-c" => config_path = Some(args.next().context("--config にファイルを指定してください")?.into()),
            "--once" => mode = Mode::Once,
            "--check" => mode = Mode::Check,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("不明な引数: {other}\n{USAGE}"),
        }
    }
    let config_path = config_path.context(USAGE)?;
    let cfg = Config::load(&config_path)?;

    let here = config::detect_platform();
    if cfg.updater.platform != here {
        bail!("この設定は {} 用ですが、実行環境は {here} です。別OS用の設定を読んでいないか確認してください", cfg.updater.platform);
    }
    let state_path = cfg.updater.state_file.clone().unwrap_or_else(|| state::default_state_dir().join("state.json"));
    let work_dir = state_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")).join("work");
    let interval = config::parse_duration(&cfg.updater.interval)?;

    if cfg.updater.request_permissions_on_start && mode == Mode::Daemon {
        service::request_permissions();
        service::request_folder_access(&cfg);
    }
    let gh = Github::new(&cfg.provider.github.token);

    let mut next_cycle = Instant::now();
    let mut attempts: HashMap<String, u32> = HashMap::new();
    loop {
        let mut state = State::load(&state_path)?;
        if Instant::now() >= next_cycle {
            let failed = cycle(&cfg, &gh, &mut state, &work_dir, mode == Mode::Check);
            if mode != Mode::Daemon {
                if failed > 0 {
                    bail!("{failed} 件の対象で失敗しました");
                }
                return Ok(());
            }
            next_cycle = Instant::now() + interval;
        } else {
            // 更新の確認とは別に、保留中の「起動」だけをやり直す
            retry_pending(&cfg, &mut state, &mut attempts);
        }
        let until_cycle = next_cycle.saturating_duration_since(Instant::now());
        let wait = if state.has_pending() { until_cycle.min(PENDING_RETRY) } else { until_cycle };
        std::thread::sleep(wait.max(Duration::from_secs(1)));
    }
}

/// 起動を保留しているターゲットのサービスだけを起動し直す。配置済みの版は再取得しない。
fn retry_pending(cfg: &Config, state: &mut State, attempts: &mut HashMap<String, u32>) {
    for t in cfg.targets.iter().filter(|t| t.enabled) {
        let Some(service) = &t.service else { continue };
        if !state.targets.get(&t.name).map_or(false, |a| a.start_pending) {
            continue;
        }
        match service::start(service) {
            Ok(()) => {
                if let Some(a) = state.targets.get_mut(&t.name) {
                    a.start_pending = false;
                }
                attempts.remove(&t.name);
                match state.save() {
                    Ok(()) => log(format!("[{}] 保留していた起動に成功しました", t.name)),
                    Err(e) => log(format!("[{}] 起動しましたが状態を保存できません: {e:#}", t.name)),
                }
            }
            Err(e) => {
                // 毎回は記録しない (初回と、その後 12 回ごと)
                let n = attempts.entry(t.name.clone()).or_insert(0);
                *n += 1;
                if *n % 12 == 1 {
                    log(format!("[{}] 起動は保留中です (許可を待っています): {e:#}", t.name));
                }
            }
        }
    }
}

/// 全ターゲットを1周する。失敗した件数を返す (1件の失敗で他を止めない)。
fn cycle(cfg: &Config, gh: &Github, state: &mut State, work_dir: &Path, check_only: bool) -> usize {
    let mut releases: HashMap<(String, Option<String>), std::result::Result<Release, String>> = HashMap::new();
    let mut failed = 0;
    for t in cfg.targets.iter().filter(|t| t.enabled) {
        if let Err(e) = update_target(cfg, gh, state, work_dir, t, &mut releases, check_only) {
            failed += 1;
            log(format!("[{}] 失敗: {e:#}", t.name));
        }
    }
    failed
}

fn update_target(
    cfg: &Config,
    gh: &Github,
    state: &mut State,
    work_dir: &Path,
    t: &Target,
    releases: &mut HashMap<(String, Option<String>), std::result::Result<Release, String>>,
    check_only: bool,
) -> Result<()> {
    let src = &cfg.source[&t.source];
    let key = (src.repo.clone(), src.release.clone());
    let rel = releases
        .entry(key)
        .or_insert_with(|| gh.release(&src.repo, src.release.as_deref()).map_err(|e| format!("{e:#}")))
        .clone()
        .map_err(anyhow::Error::msg)?;
    let asset = rel.assets.iter().find(|a| a.name == src.asset).with_context(|| format!("リリース {} に {} がありません", rel.tag_name, src.asset))?;
    let expected = gh.expected_sha256(&src.repo, &rel, asset)?;

    // 既に適用済みか
    if let Some(applied) = state.targets.get(&t.name) {
        if applied.sha256 == expected {
            log(format!("[{}] 最新です ({})", t.name, rel.tag_name));
            return Ok(());
        }
    } else if !asset.name.ends_with(".dmg") {
        // 状態が無い初回: 配置済みファイルが同一なら再起動せず記録だけする
        if let Ok(h) = github::sha256_file(&t.copy[0].to) {
            if h == expected {
                log(format!("[{}] 配置済みのファイルが最新と同一です。記録のみ行います", t.name));
                if !check_only {
                    state.targets.insert(t.name.clone(), Applied { sha256: expected, tag: rel.tag_name.clone(), start_pending: false });
                    state.save()?;
                }
                return Ok(());
            }
        }
    }

    if check_only {
        log(format!("[{}] 更新があります ({}, {} bytes)", t.name, rel.tag_name, asset.size));
        return Ok(());
    }

    log(format!("[{}] 更新を開始します ({})", t.name, rel.tag_name));
    let dl = work_dir.join(format!("{}-{}", t.name, asset.name));
    gh.download(&src.repo, asset, &dl, &expected)?;
    log(format!("[{}] ダウンロードと検証が完了しました", t.name));

    // 旧プロセスを止める前に、新しいファイルを置き場所のそばへ用意して macOS の検疫を外し、
    // 起動に使うファイルも許可しておく。ここで失敗しても旧版は動いたまま。
    let is_dmg = dl.extension().map(|e| e == "dmg").unwrap_or(false);
    let mut staged: Vec<(PathBuf, &config::Copy)> = Vec::new();
    if !is_dmg {
        for c in &t.copy {
            match install::stage_file(&dl, c) {
                Ok(p) => staged.push((p, c)),
                Err(e) => {
                    for (p, _) in &staged {
                        let _ = std::fs::remove_file(p);
                    }
                    let _ = std::fs::remove_file(&dl);
                    return Err(e.context("新しいファイルの用意に失敗 (旧版はそのままです)"));
                }
            }
        }
        log(format!("[{}] 新しいファイルを用意しました (旧プロセスはまだ動いています)", t.name));
    }
    if let Some(s) = &t.service {
        service::prepare(s);
        service::stop(s).context("サービスの停止に失敗")?;
    }
    let placed = if is_dmg {
        place_all(t, &dl, work_dir)
    } else {
        staged.iter().try_for_each(|(p, c)| install::commit_file(p, c).with_context(|| format!("{} への配置", c.to.display())))
    };
    if let Err(e) = placed {
        // 旧ファイルは残っているので、サービスだけは元に戻す
        if let Some(s) = &t.service {
            let _ = service::start(s);
        }
        for (p, _) in &staged {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_file(&dl);
        return Err(e.context("配置に失敗 (旧版のまま再起動を試みました)"));
    }
    // 再起動で自分自身が止まる場合 (reboot) に備え、起動の前に記録する
    state.targets.insert(t.name.clone(), Applied { sha256: expected, tag: rel.tag_name.clone(), start_pending: false });
    state.save()?;
    let _ = std::fs::remove_file(&dl);
    if let Some(s) = &t.service {
        if let Err(e) = service::start(s) {
            // 配置と記録は済んでいる。起動だけを保留にし、人が許可を出す(例: macOS の
            // セキュリティ)まで、起動だけを定期的に再試行する。更新のやり直しはしない。
            if let Some(a) = state.targets.get_mut(&t.name) {
                a.start_pending = true;
            }
            state.save()?;
            log(format!(
                "[{}] 更新は適用しましたが、起動を保留します。許可が出るまで {} 分ごとに起動だけを再試行します: {e:#}",
                t.name,
                PENDING_RETRY.as_secs() / 60
            ));
            return Ok(());
        }
    }
    log(format!("[{}] 更新が完了しました", t.name));
    Ok(())
}

fn place_all(t: &Target, downloaded: &Path, work_dir: &Path) -> Result<()> {
    if downloaded.extension().map(|e| e == "dmg").unwrap_or(false) {
        let dmg = install::MountedDmg::attach(downloaded, work_dir)?;
        for c in &t.copy {
            dmg.replace_app(c).with_context(|| format!("{} への配置", c.to.display()))?;
        }
        return Ok(());
    }
    for c in &t.copy {
        install::place_file(downloaded, c).with_context(|| format!("{} への配置", c.to.display()))?;
    }
    Ok(())
}
