//! 多重実行の防止。状態ファイルの隣のロックファイルに OS の排他ロック (flock / LockFileEx) を取る。
//!
//! ロックはプロセスが終了すれば (異常終了でも) OS が解放するので、「ロックファイルは残っているが
//! gh-updater のプロセスはいない」場合は取得に成功し、そのまま実行できる。ロックが取れないときは
//! 生きているプロセスが更新の最中ということなので、実行をやめる (常駐側は少し待ってやり直す)。

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, Write};
use std::path::Path;

/// 取得したロック。破棄 (drop) すると解放される。
pub struct Lock {
    _file: File,
}

/// ロックを取る。取れたら `Some`、別のプロセスが持っていれば `None` (そのプロセスの PID を中に入れて返す)。
pub fn try_acquire(path: &Path) -> Result<std::result::Result<Lock, Option<u32>>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).with_context(|| format!("ロックファイル {} を開けません", path.display()))?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0).ok();
            file.rewind().ok();
            write!(file, "{}", std::process::id()).ok();
            file.flush().ok();
            Ok(Ok(Lock { _file: file }))
        }
        Err(TryLockError::WouldBlock) => {
            let mut s = String::new();
            file.read_to_string(&mut s).ok();
            Ok(Err(s.trim().parse().ok()))
        }
        Err(TryLockError::Error(e)) => Err(e).with_context(|| format!("ロックファイル {} をロックできません", path.display())),
    }
}
