//! 設定ファイル (TOML)。未知のキーはタイプミスを見逃さないようエラーにする。
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub updater: Updater,
    #[serde(default)]
    pub provider: Providers,
    #[serde(default)]
    pub source: BTreeMap<String, Source>,
    #[serde(default, rename = "target")]
    pub targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Updater {
    /// 想定プラットフォーム。実機と違えば起動時に中止する。
    pub platform: String,
    #[serde(default = "default_interval")]
    pub interval: String,
    pub state_file: Option<PathBuf>,
    #[serde(default)]
    pub request_permissions_on_start: bool,
}

fn default_interval() -> String {
    "10m".into()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Providers {
    #[serde(default)]
    pub github: GithubProvider,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubProvider {
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub repo: String,
    pub asset: String,
    /// タグ指定。省略時は最新リリース。
    pub release: Option<String>,
}

fn default_provider() -> String {
    "github".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub name: String,
    pub source: String,
    pub copy: Vec<Copy>,
    pub service: Option<Service>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copy {
    pub to: PathBuf,
    /// 8進数のパーミッション (例 "755")。Unixのみ。
    pub mode: Option<String>,
    /// dmg 内から取り出す .app 名。
    pub entry: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Service {
    /// 任意のコマンドで停止・起動。
    Exec {
        #[serde(default)]
        stop: Vec<String>,
        #[serde(default)]
        start: Vec<String>,
    },
    DockerCompose {
        file: PathBuf,
        service: String,
    },
    /// macOS launchd (open-remote-url と同じ手順)。
    Launchd {
        label: String,
        plist: PathBuf,
        process: String,
    },
    /// macOS: Terminal で .command として動いているプログラム。
    TerminalCommand {
        process: String,
        file: PathBuf,
    },
    WindowsProcess {
        process: String,
        exe: PathBuf,
        #[serde(default)]
        args: Vec<String>,
    },
    /// 更新後に OS を再起動。
    Reboot,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("設定を読めません: {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).with_context(|| format!("設定の文法エラー: {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        parse_duration(&self.updater.interval)?;
        for (name, s) in &self.source {
            if s.provider != "github" {
                bail!("source.{name}: provider \"{}\" は未対応 (github のみ)", s.provider);
            }
            if !s.repo.contains('/') {
                bail!("source.{name}: repo は owner/repo 形式にしてください");
            }
        }
        let mut seen = std::collections::HashSet::new();
        for t in &self.targets {
            if !seen.insert(&t.name) {
                bail!("target \"{}\" が重複しています", t.name);
            }
            if !self.source.contains_key(&t.source) {
                bail!("target \"{}\": source \"{}\" が定義されていません", t.name, t.source);
            }
            if t.copy.is_empty() {
                bail!("target \"{}\": copy が空です", t.name);
            }
        }
        Ok(())
    }
}

/// "30s" "10m" "6h" "1d"
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num.parse().with_context(|| format!("時間の指定が不正です: {s:?}"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => bail!("時間の単位は s/m/h/d です: {s:?}"),
    };
    if secs == 0 {
        bail!("時間は 0 にできません");
    }
    Ok(Duration::from_secs(secs))
}

/// 実行環境のプラットフォーム名。
pub fn detect_platform() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(all(target_os = "linux", target_env = "musl", target_arch = "x86_64")) {
        "linux-x64-musl"
    } else if cfg!(all(target_os = "linux", target_env = "musl", target_arch = "aarch64")) {
        "linux-arm64-musl"
    } else if cfg!(all(target_os = "linux", target_env = "musl", target_arch = "riscv64")) {
        "linux-riscv64-musl"
    } else {
        "unsupported"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s").unwrap().as_secs(), 30);
        assert_eq!(parse_duration("10m").unwrap().as_secs(), 600);
        assert_eq!(parse_duration("6h").unwrap().as_secs(), 21600);
        assert!(parse_duration("0m").is_err());
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("m").is_err());
    }

    #[test]
    fn real_config_files_parse() {
        for f in ["updater.mac.toml", "updater.windows.toml", "updater.nanokvm.toml", "updater.webarena.toml"] {
            let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(f);
            Config::load(&p).unwrap_or_else(|e| panic!("{f}: {e:#}"));
        }
    }

    #[test]
    fn unknown_key_is_rejected() {
        let bad = "[updater]\nplatform='x'\ntypo=1\n";
        assert!(toml::from_str::<Config>(bad).is_err());
    }
}
