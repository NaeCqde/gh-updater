# gh-updater の仕様

GitHub Releases に上がった実行ファイル(や `.app` を入れた `.dmg`)を監視し、SHA-256 を検証して指定の場所へ置き、サービスの停止・起動までを1プロセスで行う常駐アップデーター。対応: Windows x64、macOS arm64、Linux x64 / arm64 / riscv64(musl)。

## コマンド

```
gh-updater --config <設定.toml> [--once | --check]
```

| 指定 | 動作 |
|---|---|
| (なし) | 常駐し、`interval` ごとに確認・更新する |
| `--once` | 1回だけ確認・更新して終了する。失敗した対象があれば終了コード 1 |
| `--check` | 更新が必要かを表示するだけ(何も変更しない) |
| `--version` / `-V`、`-h` / `--help` | 表示して終了 |

`--config` がなければ使い方を出して終了コード 1。不明な引数もエラー。

## 設定(TOML。未知のキーはエラー)

```
[updater]   platform, interval("10m"。s/m/h/d、0 は不可), state_file?, request_permissions_on_start(false)
[provider.github]  token(空でよい)
[source.<名前>]    provider("github"のみ), repo("owner/repo"), asset, release?(タグ。省略=最新)
[[target]]         enabled(true), name(重複不可), source, copy[], service?
  copy: to, mode?(8進。Unixのみ), entry?(dmg 内の .app 名)
  service: kind = exec{stop[],start[]} | docker-compose{file,service} | launchd{label,plist,process}
                | terminal-command{process,file} | windows-process{process,exe,args[]} | reboot
[self_update]      source(上の [source] の名前)
```

- `platform` が実行環境(`windows-x64` / `macos-arm64` / `linux-{x64,arm64,riscv64}-musl`)と違えば、起動時に中止する。
- `source` を持たない `target`、`copy` が空の `target` は設定の誤りとして起動しない。
- 設定にはトークンが入りうる。権限は 600 にする。

## 更新の流れ(1つの target)

1. リリース(最新、または `release` のタグ)を取り、`asset` を探す。期待する SHA-256 は、アセットの `digest`(`sha256:<hex>`)、なければ同じリリースの `<asset>.sha256` の先頭の語。
2. 状態ファイル(`state.json`)の記録と同じなら「最新」で終わる(タグが固定名 `latest` でも、中身が変わったことを SHA-256 で検知する)。記録がなく、`.dmg` でなく、置き場所のファイルが期待値と同じなら、再起動せず記録だけする。
3. ダウンロードしながら SHA-256 とサイズを検証する。違えば消して失敗にする。
4. (`.dmg` 以外)置き場所の隣に `<名前>.updater-new` として用意し、権限を整え、macOS の隔離属性(`com.apple.quarantine`)を外す。ここで失敗しても旧版は動いたまま。
5. サービスがあれば、「作業中」の印を状態に書いてから停止する。
6. 置換する(単体ファイルは `rename`。`.dmg` はマウントして `ditto` で `.app` を丸ごと入れ替える。失敗すれば元に戻す)。置換に失敗したら、サービスを旧版のまま起動し直す。
7. 適用済みとして記録してから(`reboot` で自分が止まっても分かるように)、サービスを起動する。起動に失敗したら `start_pending` を立て、更新はやり直さず、起動だけを5分ごとに再試行する(macOS の許可待ちなど、人が許可するまで通らない失敗のため)。
8. 1つの target の失敗で、ほかの target は止めない。

- 同じ `repo` と `release` のリリース情報は、1周のあいだ使い回す。
- 前回、更新の途中で止まった(「作業中」の印が残っている)target は、起動時にサービスを起動し直す。
- サービスの**停止**はベストエフォート(すでに止まっていても失敗にしない。`exec` と `docker-compose` は失敗を返す)。**起動**は失敗を報告する。

## 自己更新

`[self_update]` があれば、全 target の処理が終わったあとに行う。ダウンロードと検証 → 隣に用意して `--version` が `gh-updater ` で始まる出力を返すことを確かめる → 置換 → 記録 → 新しいバイナリで自分を起動し直す(Unix は `exec` で同じ PID、Windows は子を起こして終了)。置換の前に失敗すれば旧版のまま動く。起動保留中の target があるときは見送る。

## 状態ファイル

`state.json`(JSON): `targets`(名前 → `sha256` / `tag` / `start_pending`)と `in_progress`。書くときは一時ファイルを経由して `rename`。既定の場所は、Windows が `%LOCALAPPDATA%\gh-updater`、macOS が `~/Library/Application Support/gh-updater`、Linux が root なら `/var/lib/gh-updater`、それ以外は `~/.local/state/gh-updater`。作業用の一時ファイルはその `work/` に置く。

## 通信

GitHub API(`api.github.com`)へ。トークンがあれば `Authorization: Bearer`。リダイレクトは自動で追わず、アセットのダウンロードで返る署名付き URL には**認証ヘッダーを付けずに**手動で辿る。接続 15 秒、読み取り 60 秒で時間切れ。404 は「リポジトリ/リリースがない(非公開ならトークンが必要)」、401/403 は認証またはレート制限として報告する。

## 構成(`src/`)

`main`(周回・更新の流れ・保留の再試行)、`config`(設定)、`github`(Releases・ダウンロード・SHA-256)、`install`(配置、macOS の隔離属性、dmg)、`service`(停止・起動、macOS の許可の要求)、`self_update`、`state`、`select`(版の選び方。**まだ使われていない**: 将来のマイクラ対応用)。`service/` に常駐化のひな形(systemd、launchd、タスクスケジューラ、NanoKVM)。`updater.*.toml` は実際の設定例で、テストが読み込めることを確かめる。`updater.minecraft.example.toml` は設計メモで、現行版では読めない。

## ビルド

`cargo build --release`、`cargo test`、`./build-cross.sh`(5プラットフォーム、cross + Docker。成果物は `dist/`)。

## 脆弱性として扱わないもの・前提

- **設定ファイルは信頼する。** `exec` は設定に書かれたコマンドをそのまま実行し、`copy.to` は書かれた場所へ書く。設定を書ける人は、すでにこのユーザーの権限を持っている。
- **配布元を信頼する。** 検証するのは、リリースが公開しているハッシュ(`digest` か `.sha256`)との一致と、ダウンロードの完全さ。リリースを書き換えられる人(リポジトリの管理者)が、ハッシュごと差し替えることまでは防がない。署名の検証はしない。
- 状態ファイルは同じユーザーだけが書ける場所にある前提。
- 待ち受けるポートはない。外から受け付けるものはなく、通信は GitHub への送信だけ。
- `pkill -9 -x <process>` などの外部コマンドは、設定の `process` / `file` / `exe` の値で動く(設定を信頼する前提)。
