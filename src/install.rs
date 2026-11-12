//! 配置: 一時ファイルに書いてから置換 (途中で失敗しても旧ファイルが壊れない)。
use crate::config::Copy;
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// macOS: ダウンロード由来の検疫の印 (com.apple.quarantine) を外す。付いていなければ何もしない。
/// Gatekeeper が「開けません」と止めるのを防ぐ。外したら true。
pub fn release_quarantine(path: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let has = Command::new("xattr").args(["-p", "com.apple.quarantine"]).arg(path).output().map(|o| o.status.success()).unwrap_or(false);
    has && Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(path).status().map(|s| s.success()).unwrap_or(false)
}

/// 単体ファイルをコピー先のそばへ用意する (まだ置き換えない)。旧プロセスを止める前に呼べる。
/// パーミッションを整え、macOS の検疫を外しておく。
pub fn stage_file(src: &Path, c: &Copy) -> Result<PathBuf> {
    if let Some(p) = c.to.parent() {
        std::fs::create_dir_all(p).with_context(|| format!("フォルダを作れません: {}", p.display()))?;
    }
    let tmp = sibling(&c.to, ".updater-new");
    std::fs::copy(src, &tmp).with_context(|| format!("コピー失敗: {}", tmp.display()))?;
    set_mode(&tmp, c.mode.as_deref())?;
    release_quarantine(&tmp);
    Ok(tmp)
}

/// 用意したファイルをコピー先へ置換する。
pub fn commit_file(tmp: &Path, c: &Copy) -> Result<()> {
    std::fs::rename(tmp, &c.to).with_context(|| format!("置換失敗: {}", c.to.display()))
}

/// 単体ファイルをコピー先へ置く。
pub fn place_file(src: &Path, c: &Copy) -> Result<()> {
    let tmp = stage_file(src, c)?;
    commit_file(&tmp, c)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<&str>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(m) = mode {
        let bits = u32::from_str_radix(m, 8).map_err(|_| anyhow!("mode は8進数で指定してください: {m:?}"))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<&str>) -> Result<()> {
    Ok(())
}

/// dmg をマウントし、その中の .app をコピー先へ丸ごと入れ替える (macOS)。
pub struct MountedDmg {
    mount: PathBuf,
}

impl MountedDmg {
    pub fn attach(dmg: &Path, work: &Path) -> Result<MountedDmg> {
        let mount = work.join("mnt");
        std::fs::create_dir_all(&mount)?;
        let out = Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-readonly", "-noverify", "-mountpoint"])
            .arg(&mount)
            .arg(dmg)
            .output()
            .context("hdiutil を実行できません (macOS 専用)")?;
        if !out.status.success() {
            bail!("dmg のマウントに失敗: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(MountedDmg { mount })
    }

    pub fn replace_app(&self, c: &Copy) -> Result<()> {
        let entry = c.entry.as_deref().ok_or_else(|| anyhow!("dmg には copy.entry (.app 名) が必要です"))?;
        let src = self.mount.join(entry);
        if !src.exists() {
            bail!("dmg の中に {entry} がありません");
        }
        if let Some(p) = c.to.parent() {
            std::fs::create_dir_all(p)?;
        }
        let new = sibling(&c.to, ".updater-new");
        let old = sibling(&c.to, ".updater-old");
        let _ = std::fs::remove_dir_all(&new);
        let _ = std::fs::remove_dir_all(&old);
        // ditto は署名・拡張属性・シンボリックリンクを保って .app をコピーする
        let st = Command::new("ditto").arg(&src).arg(&new).status().context("ditto を実行できません")?;
        if !st.success() {
            let _ = std::fs::remove_dir_all(&new);
            bail!("ditto 失敗 ({entry})");
        }
        if c.to.exists() {
            std::fs::rename(&c.to, &old).with_context(|| format!("既存の .app を退避できません: {}", c.to.display()))?;
        }
        if let Err(e) = std::fs::rename(&new, &c.to) {
            let _ = std::fs::rename(&old, &c.to); // 戻す
            return Err(e).with_context(|| format!("入れ替え失敗: {}", c.to.display()));
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    }
}

impl Drop for MountedDmg {
    fn drop(&mut self) {
        let ok = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&self.mount).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            let _ = Command::new("hdiutil").args(["detach", "-force", "-quiet"]).arg(&self.mount).status();
        }
        let _ = std::fs::remove_dir(&self.mount);
    }
}
