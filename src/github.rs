//! GitHub Releases プロバイダー。大きなアセット (200MB 級) があるため、全てストリーミングで扱う。
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct Release {
    pub tag_name: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    /// "sha256:<hex>" (GitHub が付与)
    pub digest: Option<String>,
}

pub struct Github {
    agent: ureq::Agent,
    token: String,
}

impl Github {
    pub fn new(token: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .redirects(0) // 認証ヘッダを別ホスト (S3等) へ送らないため、リダイレクトは手動で辿る
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .user_agent("gh-updater")
            .build();
        Github { agent, token: token.trim().to_string() }
    }

    fn get(&self, url: &str, accept: &str) -> Result<ureq::Response> {
        let mut req = self.agent.get(url).set("Accept", accept).set("X-GitHub-Api-Version", "2022-11-28");
        if !self.token.is_empty() {
            req = req.set("Authorization", &format!("Bearer {}", self.token));
        }
        match req.call() {
            Ok(r) => Ok(r),
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                match code {
                    404 => bail!("404: リポジトリ/リリースが見つかりません (非公開ならトークンが必要): {url}"),
                    401 | 403 => bail!("{code}: 認証またはレート制限のエラー: {}", body.chars().take(200).collect::<String>()),
                    _ => bail!("HTTP {code}: {url}"),
                }
            }
            Err(e) => Err(anyhow!(e)).with_context(|| format!("通信失敗: {url}")),
        }
    }

    pub fn release(&self, repo: &str, tag: Option<&str>) -> Result<Release> {
        let url = match tag {
            Some(t) => format!("https://api.github.com/repos/{repo}/releases/tags/{t}"),
            None => format!("https://api.github.com/repos/{repo}/releases/latest"),
        };
        let text = self.get(&url, "application/vnd.github+json")?.into_string()?;
        Ok(serde_json::from_str(&text).context("リリース情報の解析に失敗")?)
    }

    /// 期待するSHA-256: アセットの digest、無ければ `<asset>.sha256` の中身。
    pub fn expected_sha256(&self, repo: &str, rel: &Release, asset: &Asset) -> Result<String> {
        if let Some(d) = &asset.digest {
            if let Some(h) = d.strip_prefix("sha256:") {
                return Ok(h.to_ascii_lowercase());
            }
        }
        let sum_name = format!("{}.sha256", asset.name);
        let sum = rel.assets.iter().find(|a| a.name == sum_name).ok_or_else(|| anyhow!("{} の SHA-256 が取得できません (digest も {sum_name} もありません)", asset.name))?;
        let mut buf = Vec::new();
        self.open_asset(repo, sum)?.take(4096).read_to_end(&mut buf)?;
        let text = String::from_utf8_lossy(&buf);
        let h = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("{sum_name} の形式が不正です");
        }
        Ok(h)
    }

    fn open_asset(&self, repo: &str, asset: &Asset) -> Result<Box<dyn Read + Send + Sync>> {
        let url = format!("https://api.github.com/repos/{repo}/releases/assets/{}", asset.id);
        let resp = self.get(&url, "application/octet-stream")?;
        if (300..400).contains(&resp.status()) {
            let loc = resp.header("Location").ok_or_else(|| anyhow!("リダイレクト先がありません"))?.to_string();
            // 署名付きURL。認証ヘッダは付けない。
            let r = self
                .agent
                .get(&loc)
                .call()
                .map_err(|e| anyhow!("ダウンロード失敗: {}", e.to_string().split('?').next().unwrap_or("")))?;
            return Ok(r.into_reader());
        }
        Ok(resp.into_reader())
    }

    /// ディスクへストリーミングしつつ SHA-256 を計算し、期待値と照合する。
    pub fn download(&self, repo: &str, asset: &Asset, dest: &Path, expected_sha256: &str) -> Result<()> {
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut reader = self.open_asset(repo, asset)?;
        let mut file = std::fs::File::create(dest).with_context(|| format!("作成できません: {}", dest.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            total += n as u64;
        }
        file.flush()?;
        let got = hex(&hasher.finalize());
        if got != expected_sha256 {
            let _ = std::fs::remove_file(dest);
            bail!("SHA-256 が一致しません ({}): 期待 {expected_sha256} / 実際 {got}", asset.name);
        }
        if asset.size != 0 && total != asset.size {
            let _ = std::fs::remove_file(dest);
            bail!("サイズが一致しません ({}): 期待 {} / 実際 {total}", asset.name, asset.size);
        }
        Ok(())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}
