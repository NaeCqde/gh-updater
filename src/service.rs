//! サービスの停止・起動。停止は「ベストエフォート」(既に止まっていても失敗にしない)、起動は失敗を報告する。
use crate::config::Service;
use anyhow::{anyhow, bail, Context, Result};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

fn run(argv: &[String]) -> Result<()> {
    let (prog, args) = argv.split_first().ok_or_else(|| anyhow!("コマンドが空です"))?;
    let st = Command::new(prog).args(args).status().with_context(|| format!("実行できません: {prog}"))?;
    if !st.success() {
        bail!("{} が失敗しました ({})", argv.join(" "), st);
    }
    Ok(())
}

fn run_str(prog: &str, args: &[&str]) -> Result<()> {
    run(&std::iter::once(prog).chain(args.iter().copied()).map(String::from).collect::<Vec<_>>())
}

fn quiet(prog: &str, args: &[&str]) {
    let _ = Command::new(prog).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}

pub fn stop(s: &Service) -> Result<()> {
    match s {
        Service::Exec { stop, .. } => {
            if !stop.is_empty() {
                run(stop)?;
            }
        }
        Service::DockerCompose { file, service } => {
            run_str("docker", &["compose", "-f", &file.to_string_lossy(), "stop", service])?;
        }
        Service::Launchd { plist, process, .. } => {
            quiet("launchctl", &["unload", &plist.to_string_lossy()]);
            // open -a 経由で起動された残りプロセスも掃除する (open-remote-url と同じ)
            quiet("pkill", &["-9", "-x", process]);
        }
        Service::TerminalCommand { process, .. } => stop_terminal_command(process),
        Service::WindowsProcess { process, .. } => {
            quiet("taskkill", &["/F", "/IM", process]);
            sleep(Duration::from_millis(800)); // exe のロック解放待ち
        }
        Service::Reboot => {}
    }
    Ok(())
}

pub fn start(s: &Service) -> Result<()> {
    match s {
        Service::Exec { start, .. } => {
            if !start.is_empty() {
                run(start)?;
            }
        }
        Service::DockerCompose { file, service } => {
            run_str("docker", &["compose", "-f", &file.to_string_lossy(), "up", "-d", service])?;
        }
        Service::Launchd { label, plist, .. } => {
            run_str("launchctl", &["load", &plist.to_string_lossy()])?;
            run_str("launchctl", &["start", label])?;
        }
        Service::TerminalCommand { file, .. } => {
            release_launcher(file);
            run_str("open", &[&file.to_string_lossy()])?;
        }
        Service::WindowsProcess { exe, args, .. } => spawn_detached(exe, args)?,
        Service::Reboot => {
            if cfg!(windows) {
                run_str("shutdown", &["/r", "/t", "5"])?;
            } else {
                run_str("reboot", &[])?;
            }
        }
    }
    Ok(())
}

/// 起動に使う .command の検疫の印を外す (macOS)。付いたままだと Gatekeeper が
/// 「"○○.command" を開けません」と止める。起動の直前と、旧プロセスを止める前に呼ぶ。
pub fn release_launcher(file: &std::path::Path) {
    if crate::install::release_quarantine(file) {
        eprintln!("検疫の印を外しました: {}", file.display());
    }
}

/// 旧プロセスを止める前の準備 (起動に使うファイルの許可)。
pub fn prepare(s: &Service) {
    if let Service::TerminalCommand { file, .. } = s {
        release_launcher(file);
    }
}

#[cfg(windows)]
fn spawn_detached(exe: &std::path::Path, args: &[String]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(exe).args(args).creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW).spawn().with_context(|| format!("起動できません: {}", exe.display()))?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached(exe: &std::path::Path, args: &[String]) -> Result<()> {
    Command::new(exe).args(args).spawn().with_context(|| format!("起動できません: {}", exe.display()))?;
    Ok(())
}

/// macOS: そのプロセスの tty を持つ Terminal のウィンドウごと消す。
/// 初回は「オートメーション」の許可ダイアログが出る。
fn stop_terminal_command(process: &str) {
    let tty = Command::new("pgrep")
        .args(["-x", process])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        .and_then(|pid| Command::new("ps").args(["-o", "tty=", "-p", &pid]).output().ok())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "??");

    // 穏やかに終了を依頼し、終わるまで待つ。実測では best-lights-selector の停止に約2秒かかり、
    // 待たずにウィンドウを閉じると「プロセス実行中」の確認で閉じられず、旧ウィンドウが残った。
    quiet("pkill", &["-x", process]);
    if !wait_gone(process, Duration::from_secs(15)) {
        quiet("pkill", &["-9", "-x", process]);
        wait_gone(process, Duration::from_secs(3));
    }
    sleep(Duration::from_millis(300));

    if let Some(tty) = tty {
        let script = format!(
            r#"tell application "Terminal"
  set targets to {{}}
  repeat with w in (windows as list)
    try
      repeat with t in (tabs of w as list)
        if (tty of t) is "/dev/{tty}" then set end of targets to w
      end repeat
    end try
  end repeat
  repeat with w in targets
    try
      close w
    end try
  end repeat
end tell"#
        );
        quiet("osascript", &["-e", &script]);
    }
}

fn wait_gone(process: &str, max: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < max {
        let alive = Command::new("pgrep").args(["-x", process]).output().map(|o| !o.stdout.is_empty()).unwrap_or(false);
        if !alive {
            return true;
        }
        sleep(Duration::from_millis(200));
    }
    false
}

/// 初回起動時に Terminal 操作の許可ダイアログを出す (macOS)。
pub fn request_permissions() {
    if cfg!(target_os = "macos") {
        eprintln!("Terminal の操作許可を確認します (ダイアログが出たら許可してください)");
        let ok = Command::new("osascript").args(["-e", r#"tell application "Terminal" to count windows"#]).output().map(|o| o.status.success()).unwrap_or(false);
        eprintln!("{}", if ok { "許可済みです" } else { "許可されていません (システム設定 → プライバシーとセキュリティ → オートメーション)" });
    }
}

/// 設定が触るフォルダ (置き場所・起動に使うファイルの親) を集める。重複は除く。
pub fn folders_in_use(cfg: &crate::config::Config) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut add = |p: &std::path::Path| {
        if let Some(dir) = p.parent() {
            if !out.iter().any(|d| d == dir) {
                out.push(dir.to_path_buf());
            }
        }
    };
    for t in cfg.targets.iter().filter(|t| t.enabled) {
        for c in &t.copy {
            add(&c.to);
        }
        match &t.service {
            Some(Service::TerminalCommand { file, .. }) => add(file),
            Some(Service::Launchd { plist, .. }) => add(plist),
            Some(Service::DockerCompose { file, .. }) => add(file),
            _ => {}
        }
    }
    out
}

/// 起動時に、設定が触るフォルダへ実際にアクセスして、macOS の「フォルダへのアクセス」の
/// 確認を更新の途中ではなく先にまとめて出す。許可は実行ファイルの署名が同じ間は残る。
/// 設定に無いフォルダには触れない (不要な許可を求めない)。
pub fn request_folder_access(cfg: &crate::config::Config) {
    if !cfg!(target_os = "macos") {
        return;
    }
    for dir in folders_in_use(cfg) {
        // 読み取りで一覧を試し、書き込みの確認も同じフォルダへの許可なので一度で済む
        let ok = std::fs::read_dir(&dir).is_ok();
        eprintln!("フォルダへのアクセス {} => {}", dir.display(), if ok { "許可済みです" } else { "許可されていません (システム設定 → プライバシーとセキュリティ → ファイルとフォルダ)" });
    }
}
