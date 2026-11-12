//! gh-updater 自身の更新。
//!
//! 他の対象の更新を中断しないよう、必ず周回の最後 (全対象の処理後) に行う。
//! 手順: ダウンロード+検証 → 隣に用意して `--version` で実行できることを確認 →
//! 実行ファイルを置換 → 状態を記録 → 新しいバイナリで自分を起動し直す。
//! 置換前に失敗すれば旧版のまま動き続ける。
use crate::config::{Config, SelfUpdate};
use crate::github::{self, Github};
use crate::install;
use crate::state::{Applied, State};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const KEY: &str = "@self";

fn log(msg: impl AsRef<str>) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    eprintln!("[{secs}] {}", msg.as_ref());
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

pub fn run(cfg: &Config, su: &SelfUpdate, gh: &Github, state: &mut State, work_dir: &Path, check_only: bool, relaunch: bool) -> Result<()> {
    let src = &cfg.source[&su.source];
    let rel = gh.release(&src.repo, src.release.as_deref())?;
    let asset = rel.assets.iter().find(|a| a.name == src.asset).with_context(|| format!("リリース {} に {} がありません", rel.tag_name, src.asset))?;
    let expected = gh.expected_sha256(&src.repo, &rel, asset)?;
    let exe = std::env::current_exe().context("自分の実行ファイルの場所を取得できません")?;

    match state.targets.get(KEY) {
        Some(a) if a.sha256 == expected => {
            log(format!("[自己更新] 最新です ({})", rel.tag_name));
            return Ok(());
        }
        None if github::sha256_file(&exe).map(|h| h == expected).unwrap_or(false) => {
            log("[自己更新] 実行中のファイルが最新と同一です。記録のみ行います");
            if !check_only {
                state.targets.insert(KEY.into(), Applied { sha256: expected, tag: rel.tag_name, start_pending: false });
                state.save()?;
            }
            return Ok(());
        }
        _ => {}
    }
    if check_only {
        log(format!("[自己更新] 更新があります ({}, {} bytes)", rel.tag_name, asset.size));
        return Ok(());
    }
    if state.has_pending() {
        log("[自己更新] 起動が保留中の対象があるため、自己更新は見送ります");
        return Ok(());
    }

    log(format!("[自己更新] 更新を開始します ({})", rel.tag_name));
    let dl = work_dir.join(format!("self-{}", asset.name));
    gh.download(&src.repo, asset, &dl, &expected)?;

    // 実行ファイルの隣に用意し、確実に動くものだけを置換する (別アーキの取り違え等を防ぐ)
    let staged = sibling(&exe, ".updater-new");
    let prepared = (|| -> Result<()> {
        std::fs::copy(&dl, &staged).with_context(|| format!("コピー失敗: {}", staged.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        install::release_quarantine(&staged);
        let out = std::process::Command::new(&staged).arg("--version").output().context("新しいバイナリを実行できません")?;
        if !out.status.success() || !String::from_utf8_lossy(&out.stdout).starts_with("gh-updater ") {
            bail!("新しいバイナリが --version に正しく応答しません");
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&dl);
    if let Err(e) = prepared {
        let _ = std::fs::remove_file(&staged);
        return Err(e.context("新しい版の確認に失敗 (旧版のまま動きます)"));
    }

    swap(&staged, &exe)?;
    state.targets.insert(KEY.into(), Applied { sha256: expected, tag: rel.tag_name.clone(), start_pending: false });
    state.save()?;
    log(format!("[自己更新] {} に更新しました", rel.tag_name));

    if relaunch {
        log("[自己更新] 新しい版で起動し直します");
        restart_self(&exe);
    }
    Ok(())
}

#[cfg(unix)]
fn swap(staged: &Path, exe: &Path) -> Result<()> {
    // 実行中でも rename で置換できる (旧 inode は終了まで生きる)
    std::fs::rename(staged, exe).with_context(|| format!("置換失敗: {}", exe.display()))
}

#[cfg(windows)]
fn swap(staged: &Path, exe: &Path) -> Result<()> {
    // 実行中の exe は上書き・削除できないが、名前の変更はできる
    let old = sibling(exe, ".updater-old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).context("実行中の exe を退避できません")?;
    if let Err(e) = std::fs::rename(staged, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e).context("置換失敗 (元に戻しました)");
    }
    Ok(())
}

#[cfg(unix)]
fn restart_self(exe: &Path) -> ! {
    use std::os::unix::process::CommandExt;
    // 同じ PID のまま新しいバイナリに置き換わる (systemd / launchd からは再起動に見えない)
    let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
    log(format!("[自己更新] 再実行に失敗しました。終了して常駐管理に任せます: {err}"));
    std::process::exit(1);
}

#[cfg(windows)]
fn restart_self(exe: &Path) -> ! {
    match std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            log(format!("[自己更新] 再起動に失敗しました。終了して常駐管理に任せます: {e}"));
            std::process::exit(1);
        }
    }
}
