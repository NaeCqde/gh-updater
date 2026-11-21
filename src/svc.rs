//! 自分自身を OS の常駐に登録・解除する (`--install` / `--uninstall`)。
//!
//! - Linux (systemd): root なら `/etc/systemd/system/gh-updater.service`、一般ユーザーなら `~/.config/systemd/user/`
//! - init.d しかない Linux (BusyBox など): `/etc/init.d/S99gh-updater.sh` (root のみ)
//! - macOS: `~/Library/LaunchAgents/quest.nae.gh-updater.plist` (Terminal 操作のためログインユーザーの LaunchAgent)
//! - Windows: タスクスケジューラの `gh-updater` (ログオン時に起動、落ちたら1分後に再起動)
//!
//! 実行ファイルと設定ファイルは「いまの実行ファイル」「--config で渡された設定」の絶対パスを書き込む。
//! 解除しても実行ファイル・設定・状態ファイルは消さない。`--dry-run` なら書く内容と実行するコマンドを表示するだけ。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const NAME: &str = "gh-updater";
const LABEL: &str = "quest.nae.gh-updater";

#[derive(Debug, PartialEq)]
enum Kind {
    SystemdSystem,
    SystemdUser,
    InitD,
    Launchd,
    WindowsTask,
}

fn detect() -> Result<Kind> {
    if cfg!(target_os = "windows") {
        return Ok(Kind::WindowsTask);
    }
    if cfg!(target_os = "macos") {
        return Ok(Kind::Launchd);
    }
    let root = is_root();
    if Path::new("/run/systemd/system").exists() {
        return Ok(if root { Kind::SystemdSystem } else { Kind::SystemdUser });
    }
    if root && Path::new("/etc/init.d").is_dir() {
        return Ok(Kind::InitD);
    }
    bail!("この環境の常駐方式 (systemd / init.d) を判別できません。service/ のひな形を参考に手で登録してください")
}

fn is_root() -> bool {
    Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0").unwrap_or(false)
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).context("ホームディレクトリが分かりません")
}

fn user_id() -> String {
    Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// 書き込み先と内容。
struct Unit {
    path: PathBuf,
    content: String,
    mode: u32,
}

fn unit_for(kind: &Kind, exe: &Path, cfg: &Path) -> Result<Option<Unit>> {
    Ok(match kind {
        Kind::SystemdSystem => Some(Unit { path: PathBuf::from("/etc/systemd/system/gh-updater.service"), content: systemd_unit(exe, cfg, "multi-user.target"), mode: 0o644 }),
        Kind::SystemdUser => Some(Unit { path: home()?.join(".config/systemd/user/gh-updater.service"), content: systemd_unit(exe, cfg, "default.target"), mode: 0o644 }),
        Kind::InitD => Some(Unit { path: PathBuf::from("/etc/init.d/S99gh-updater.sh"), content: init_script(exe, cfg), mode: 0o755 }),
        Kind::Launchd => {
            let h = home()?;
            Some(Unit { path: h.join(format!("Library/LaunchAgents/{LABEL}.plist")), content: plist(exe, cfg, &h.join("Library/Logs/gh-updater.log")), mode: 0o644 })
        }
        Kind::WindowsTask => None,
    })
}

pub fn systemd_unit(exe: &Path, cfg: &Path, wanted_by: &str) -> String {
    format!(
        "[Unit]\nDescription=GitHub release auto updater\nAfter=network-online.target docker.service\nWants=network-online.target\n\n[Service]\nExecStart={} --config {}\nRestart=always\nRestartSec=30\nMemoryMax=64M\n\n[Install]\nWantedBy={wanted_by}\n",
        systemd_quote(exe),
        systemd_quote(cfg)
    )
}

fn systemd_quote(p: &Path) -> String {
    let s = p.display().to_string();
    if s.contains(' ') || s.contains('"') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s
    }
}

pub fn init_script(exe: &Path, cfg: &Path) -> String {
    let (exe, cfg) = (sh_quote(exe), sh_quote(cfg));
    // 時計が合っていない機器 (RTC なし: 起動直後は 1970 年) では TLS が失敗するので、年が妥当になるまで待ってから起動する (最大5分)
    format!(
        "#!/bin/sh\n# gh-updater --install が作成 (BusyBox init)\nEXE={exe}\nCFG={cfg}\nPID=/var/run/gh-updater.pid\ncase \"$1\" in\n  start)\n    (\n      i=0\n      while [ \"$(date +%Y)\" -lt 2024 ] && [ $i -lt 150 ]; do sleep 2; i=$((i+1)); done\n      start-stop-daemon -S -b -m -p \"$PID\" -x \"$EXE\" -- --config \"$CFG\"\n    ) &\n    ;;\n  stop)    start-stop-daemon -K -p \"$PID\" ;;\n  restart) \"$0\" stop; \"$0\" start ;;\nesac\n"
    )
}

fn sh_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

pub fn plist(exe: &Path, cfg: &Path, log: &Path) -> String {
    let x = |p: &Path| p.display().to_string().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{LABEL}</string>\n    <key>ProgramArguments</key>\n    <array>\n        <string>{}</string>\n        <string>--config</string>\n        <string>{}</string>\n    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <true/>\n    <key>StandardErrorPath</key>\n    <string>{}</string>\n</dict>\n</plist>\n",
        x(exe),
        x(cfg),
        x(log)
    )
}

fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

pub fn windows_register_script(exe: &Path, cfg: &Path) -> String {
    format!(
        "$exe = {}; $cfg = {}; \
         $action = New-ScheduledTaskAction -Execute $exe -Argument ('--config \"' + $cfg + '\"'); \
         $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
         $settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries; \
         Register-ScheduledTask -TaskName '{NAME}' -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null; \
         Start-ScheduledTask -TaskName '{NAME}'",
        ps_quote(exe),
        ps_quote(cfg)
    )
}

const WINDOWS_UNREGISTER: &str = "Stop-ScheduledTask -TaskName 'gh-updater' -ErrorAction SilentlyContinue; Unregister-ScheduledTask -TaskName 'gh-updater' -Confirm:$false";

struct Runner {
    dry: bool,
}

impl Runner {
    /// コマンドを実行する。`must` が false なら失敗しても続ける (すでに無いものを止めるとき等)。
    fn run(&self, must: bool, prog: &str, args: &[&str]) -> Result<()> {
        println!("$ {prog} {}", args.join(" "));
        if self.dry {
            return Ok(());
        }
        match Command::new(prog).args(args).status() {
            Ok(s) if s.success() => Ok(()),
            Ok(s) if !must => {
                println!("  (終了コード {s}: 無視します)");
                Ok(())
            }
            Ok(s) => bail!("{prog} が失敗しました ({s})"),
            Err(e) if !must => {
                println!("  ({e}: 無視します)");
                Ok(())
            }
            Err(e) => Err(e).with_context(|| format!("{prog} を実行できません")),
        }
    }

    fn write(&self, u: &Unit) -> Result<()> {
        println!("{} を作成します:\n{}", u.path.display(), u.content);
        if self.dry {
            return Ok(());
        }
        if let Some(dir) = u.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("{} を作れません", dir.display()))?;
        }
        std::fs::write(&u.path, &u.content).with_context(|| format!("{} を書けません", u.path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&u.path, std::fs::Permissions::from_mode(u.mode)).ok();
        }
        let _ = u.mode;
        Ok(())
    }

    fn remove(&self, path: &Path) -> Result<()> {
        println!("{} を削除します", path.display());
        if self.dry || !path.exists() {
            return Ok(());
        }
        std::fs::remove_file(path).with_context(|| format!("{} を削除できません", path.display()))
    }
}

pub fn install(config: &Path, dry: bool) -> Result<()> {
    let exe = std::env::current_exe().context("実行ファイルの場所が分かりません")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let cfg = std::fs::canonicalize(config).with_context(|| format!("設定ファイル {} が見つかりません", config.display()))?;
    let kind = detect()?;
    let r = Runner { dry };
    let unit = unit_for(&kind, &exe, &cfg)?;
    if let Some(u) = &unit {
        r.write(u)?;
    }
    match kind {
        Kind::SystemdSystem => {
            r.run(true, "systemctl", &["daemon-reload"])?;
            r.run(true, "systemctl", &["enable", "--now", NAME])?;
        }
        Kind::SystemdUser => {
            r.run(true, "systemctl", &["--user", "daemon-reload"])?;
            r.run(true, "systemctl", &["--user", "enable", "--now", NAME])?;
            println!("注意: ログアウト後も動かすには `loginctl enable-linger $USER` が必要です");
        }
        Kind::InitD => {
            let script = unit.as_ref().map(|u| u.path.display().to_string()).unwrap_or_default();
            r.run(false, &script, &["stop"])?;
            r.run(true, &script, &["start"])?;
        }
        Kind::Launchd => {
            let domain = format!("gui/{}", user_id());
            let target = format!("{domain}/{LABEL}");
            let plist = unit.as_ref().map(|u| u.path.display().to_string()).unwrap_or_default();
            r.run(false, "launchctl", &["bootout", &target])?;
            r.run(true, "launchctl", &["bootstrap", &domain, &plist])?;
            println!("注意: 初回は Terminal で `gh-updater --config <設定>` を手動起動し、オートメーションを許可してください");
        }
        Kind::WindowsTask => {
            r.run(true, "powershell", &["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &windows_register_script(&exe, &cfg)])?;
        }
    }
    if dry {
        println!("--dry-run: 何も変更していません");
    } else {
        println!("登録しました (実行ファイル: {}, 設定: {})", exe.display(), cfg.display());
    }
    Ok(())
}

pub fn uninstall(dry: bool) -> Result<()> {
    let kind = detect()?;
    let r = Runner { dry };
    // 解除では内容は使わないので、パスだけ分かればよい
    let unit = unit_for(&kind, Path::new(NAME), Path::new(NAME))?;
    match kind {
        Kind::SystemdSystem => {
            r.run(false, "systemctl", &["disable", "--now", NAME])?;
            if let Some(u) = &unit {
                r.remove(&u.path)?;
            }
            r.run(true, "systemctl", &["daemon-reload"])?;
        }
        Kind::SystemdUser => {
            r.run(false, "systemctl", &["--user", "disable", "--now", NAME])?;
            if let Some(u) = &unit {
                r.remove(&u.path)?;
            }
            r.run(true, "systemctl", &["--user", "daemon-reload"])?;
        }
        Kind::InitD => {
            if let Some(u) = &unit {
                r.run(false, &u.path.display().to_string(), &["stop"])?;
                r.remove(&u.path)?;
            }
        }
        Kind::Launchd => {
            let target = format!("gui/{}/{LABEL}", user_id());
            r.run(false, "launchctl", &["bootout", &target])?;
            if let Some(u) = &unit {
                r.remove(&u.path)?;
            }
        }
        Kind::WindowsTask => {
            r.run(true, "powershell", &["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", WINDOWS_UNREGISTER])?;
        }
    }
    if dry {
        println!("--dry-run: 何も変更していません");
    } else {
        println!("解除しました (実行ファイル・設定・状態ファイルは残してあります)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_unit_quotes_paths_with_spaces() {
        let u = systemd_unit(Path::new("/usr/local/bin/gh-updater"), Path::new("/etc/my dir/u.toml"), "multi-user.target");
        assert!(u.contains("ExecStart=/usr/local/bin/gh-updater --config \"/etc/my dir/u.toml\"\n"));
        assert!(u.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn init_script_quotes_single_quotes() {
        let s = init_script(Path::new("/opt/it's/gh-updater"), Path::new("/opt/u.toml"));
        assert!(s.contains("EXE='/opt/it'\\''s/gh-updater'"));
    }

    #[test]
    fn plist_escapes_xml() {
        let p = plist(Path::new("/a&b/gh-updater"), Path::new("/c.toml"), Path::new("/l.log"));
        assert!(p.contains("/a&amp;b/gh-updater"));
        assert!(p.contains(LABEL));
    }

    #[test]
    fn windows_script_escapes_quotes() {
        let s = windows_register_script(Path::new(r"C:\Users\o'neil\gh-updater.exe"), Path::new(r"C:\c.toml"));
        assert!(s.contains(r"'C:\Users\o''neil\gh-updater.exe'"));
        assert!(s.contains("Register-ScheduledTask"));
    }
}
