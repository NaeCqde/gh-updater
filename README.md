# gh-updater

GitHub Releases に上がった実行ファイル(や `.app.dmg`)を監視し、SHA-256 を検証して指定フォルダへ配置し、サービスの停止・起動までを 1 プロセスで行う常駐アップデーターです。設定は TOML。

- 対応: Windows x64 / macOS arm64 / Linux x64・arm64・riscv64 (musl)
- 1 つのファイルを複数フォルダへコピー可能、対象(target)を複数まとめて監視
- 再起動方式: `exec` / `docker-compose` / `launchd` / `terminal-command` / `windows-process` / `reboot`
- 変更検出はアセットの `digest`(sha256)を状態ファイルと比較(タグが固定でも検知可能)

- 自己更新: `[self_update]` を書くと gh-updater 自身も更新(全対象の更新後に実行、`--version` で動作確認してから置換し再起動)。更新が途中で中断されても、次の起動時にサービスを起動し直す

## 使い方

```bash
gh-updater --config updater.toml          # 常駐(interval ごとに確認)
gh-updater --config updater.toml --once   # 1 回だけ確認・更新
gh-updater --config updater.toml --check  # 確認のみ(更新しない)
```

設定例は `updater.*.toml`(冒頭のコメントに構造の説明あり)。常駐化のひな形は `service/`(systemd / launchd / タスクスケジューラ / NanoKVM)。

## ビルド

```bash
cargo build --release   # ホスト用
./build-cross.sh        # 5 プラットフォーム(cross + Docker)。成果物は dist/
```

`updater.minecraft.example.toml` は将来のマイクラ jar 自動更新の設計メモで、現行版では読み込めません。
