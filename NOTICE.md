# NOTICE

Tote は次のオープンソースのライブラリを使っています。

- **unrar / unrar_sys**（MIT OR Apache-2.0）— RARの展開。
  内部では RARLAB の UnRAR ソースコードを使用しています。UnRAR のソースは、RAR 書庫を扱うソフトに組み込んで自由に使えますが、
  **RAR の圧縮アルゴリズムを再現する目的には使えません**。Tote は RAR の展開のみを行い、RAR の作成機能は持ちません。
  詳細: https://www.rarlab.com/
- **zip**, **sevenz-rust2**, **cab**, **delharc**, **tar**, **flate2** — ZIP / 7z / CAB / LZH / tar / gzip の読み書き。
- **tauri**, **drag**, **encoding_rs**, **serde**, **time** ほか — 画面・ドラッグ・文字コード変換など。

各ライブラリのライセンスは、それぞれの配布元（crates.io）を参照してください。
