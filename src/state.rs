//! 適用済みの版 (アセットの SHA-256) を記録する。タグが固定名 "latest" でも更新を検出できる。
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub targets: BTreeMap<String, Applied>,
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applied {
    pub sha256: String,
    pub tag: String,
    /// 更新(配置)は済んでいるが、サービスの起動に失敗して保留中。macOS のセキュリティの
    /// 許可待ちなど、人が許可するまで通らない失敗でも、許可後に起動だけを再試行できる。
    #[serde(default)]
    pub start_pending: bool,
}

impl State {
    pub fn has_pending(&self) -> bool {
        self.targets.values().any(|a| a.start_pending)
    }

    pub fn load(path: &Path) -> Result<State> {
        let mut s: State = match std::fs::read_to_string(path) {
            Ok(t) => serde_json::from_str(&t).with_context(|| format!("状態ファイルが壊れています: {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e).with_context(|| format!("状態ファイルを読めません: {}", path.display())),
        };
        s.path = path.to_path_buf();
        Ok(s)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// 状態の置き場所 (設定で state_file を指定しなければ OS 標準)。
pub fn default_state_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("gh-updater")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("Library/Application Support/gh-updater")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if std::env::var("USER").map(|u| u == "root").unwrap_or(false) {
            PathBuf::from("/var/lib/gh-updater")
        } else {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/state/gh-updater")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_state_without_start_pending_still_loads() {
        let dir = std::env::temp_dir().join(format!("gh-updater-state-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, r#"{"targets":{"a":{"sha256":"x","tag":"latest"}}}"#).unwrap();
        let mut s = State::load(&path).unwrap();
        assert!(!s.targets["a"].start_pending);
        assert!(!s.has_pending());
        s.targets.get_mut("a").unwrap().start_pending = true;
        s.save().unwrap();
        assert!(State::load(&path).unwrap().has_pending());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
