//! 汎用バージョンセレクター。GitHub 以外 (Minecraft サーバー本体・プラグイン等) でも使えるよう、
//! 配布元に依存しない形にしてある。まだ main からは使っていない (Minecraft 対応で使用予定)。
#![allow(dead_code)]
use std::cmp::Ordering;

/// 安定度。数値が大きいほど安定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Alpha = 0,
    Beta = 1,
    Stable = 2,
}

/// 設定上の許容下限: "stable" "beta" "alpha" "any"。
pub fn parse_floor(s: &str) -> Option<Channel> {
    match s {
        "stable" => Some(Channel::Stable),
        "beta" => Some(Channel::Beta),
        "alpha" | "any" => Some(Channel::Alpha),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub version: String,
    pub channel: Channel,
    /// この候補が対応を宣言している相手側の版 (例: プラグインの対応Minecraft版)。
    pub compatible_with: Vec<String>,
}

/// 数値の並び。"v1.21.4" "1.21.4-SNAPSHOT" の数値部分を取る。
pub fn numeric_parts(v: &str) -> Vec<u64> {
    let v = v.trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next().unwrap_or(v);
    core.split('.').map_while(|p| p.parse::<u64>().ok()).collect()
}

fn cmp_versions(a: &str, b: &str) -> Ordering {
    let (x, y) = (numeric_parts(a), numeric_parts(b));
    let n = x.len().max(y.len());
    for i in 0..n {
        let c = x.get(i).unwrap_or(&0).cmp(y.get(i).unwrap_or(&0));
        if c != Ordering::Equal {
            return c;
        }
    }
    Ordering::Equal
}

/// "latest" / "1.21.4" (完全一致) / "1.21.x" ("x" の桁は最大のものを選ぶ)
pub fn matches_pattern(version: &str, pattern: &str) -> bool {
    if pattern == "latest" {
        return true;
    }
    let want: Vec<&str> = pattern.trim_start_matches(['v', 'V']).split('.').collect();
    let have = numeric_parts(version);
    let wildcard = matches!(want.last(), Some(&"x") | Some(&"X") | Some(&"*"));
    let fixed = if wildcard { &want[..want.len() - 1] } else { &want[..] };
    if wildcard {
        if have.len() <= fixed.len() {
            return false;
        }
    } else {
        // 完全一致: 末尾の 0 は省略可 ("1.21" == "1.21.0")
        let want_n: Vec<u64> = match fixed.iter().map(|w| w.parse::<u64>()).collect::<Result<_, _>>() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let n = want_n.len().max(have.len());
        return (0..n).all(|i| want_n.get(i).unwrap_or(&0) == have.get(i).unwrap_or(&0));
    }
    fixed.iter().enumerate().all(|(i, w)| w.parse::<u64>().ok() == have.get(i).copied())
}

/// 条件に合う最大の版を選ぶ。`compatible` があれば、その版を宣言している候補だけ。
pub fn select<'a>(
    candidates: &'a [Candidate],
    floor: Channel,
    pattern: &str,
    compatible: Option<&str>,
) -> Option<&'a Candidate> {
    candidates
        .iter()
        .filter(|c| c.channel >= floor)
        .filter(|c| matches_pattern(&c.version, pattern))
        .filter(|c| match compatible {
            Some(target) => c.compatible_with.iter().any(|w| w == target),
            None => true,
        })
        .max_by(|a, b| cmp_versions(&a.version, &b.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: &str, ch: Channel, compat: &[&str]) -> Candidate {
        Candidate { version: v.into(), channel: ch, compatible_with: compat.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn x_picks_numerically_largest() {
        let list = vec![c("1.21.4", Channel::Stable, &[]), c("1.21.11", Channel::Stable, &[]), c("1.20.6", Channel::Stable, &[]), c("1.22.0", Channel::Stable, &[])];
        assert_eq!(select(&list, Channel::Stable, "1.21.x", None).unwrap().version, "1.21.11");
        assert_eq!(select(&list, Channel::Stable, "latest", None).unwrap().version, "1.22.0");
        assert_eq!(select(&list, Channel::Stable, "1.20.6", None).unwrap().version, "1.20.6");
        assert!(select(&list, Channel::Stable, "1.19.x", None).is_none());
    }

    #[test]
    fn channel_floor() {
        let list = vec![c("2.0.0", Channel::Beta, &[]), c("1.9.0", Channel::Stable, &[])];
        assert_eq!(select(&list, Channel::Stable, "latest", None).unwrap().version, "1.9.0");
        assert_eq!(select(&list, Channel::Beta, "latest", None).unwrap().version, "2.0.0");
        assert_eq!(select(&list, parse_floor("any").unwrap(), "latest", None).unwrap().version, "2.0.0");
    }

    #[test]
    fn plugin_follows_server_version() {
        let plugin = vec![c("5.1", Channel::Stable, &["1.21.4", "1.21.5"]), c("5.2", Channel::Stable, &["1.21.5", "1.21.6"]), c("5.3", Channel::Stable, &["1.22"])];
        assert_eq!(select(&plugin, Channel::Stable, "latest", Some("1.21.4")).unwrap().version, "5.1");
        assert_eq!(select(&plugin, Channel::Stable, "latest", Some("1.21.5")).unwrap().version, "5.2");
        assert!(select(&plugin, Channel::Stable, "latest", Some("1.21.11")).is_none());
    }

    #[test]
    fn wildcard_needs_a_deeper_level() {
        assert!(matches_pattern("1.21.4", "1.21.x"));
        assert!(!matches_pattern("1.21", "1.21.x"));
        assert!(matches_pattern("1.21", "1.21.0"));
        assert!(matches_pattern("3.5.0-SNAPSHOT-594", "3.5.x"));
    }
}
