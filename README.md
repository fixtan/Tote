# Tote

何でも放り込んで持ち運ぶ、自作のWinRAR代替アーカイバ。

- v0.1.0: ZIP作成（右クリック / 送る / D&D）
- v0.2.0: ZIPビューア（展開前に中身を閲覧、ドラッグで個別展開）

## ビルド (Windows)
    cargo build --release
    → target\release\tote.exe（初回は Tauri の依存取得で時間がかかる。WebView2 が必要（Win10/11は通常入っている））

## 使い方
1. `tote.exe` をダブルクリック → 「はい」で右クリックメニュー・「送る」・.zip の「Toteで開く」を登録（HKCU のみ、管理者権限不要）
2. 圧縮: 右クリック → (Win11は「その他のオプションを確認」) → ToteでZIPに圧縮 / 15個超は 送る → ToteでZIPに圧縮 / D&D
3. 閲覧: .zip を右クリック → Toteで開く、または `tote.exe --open file.zip`
4. ダブルクリックで開きたい場合: .zip を右クリック → プログラムから開く → Tote → 「常に使う」（Windows の仕様で手動設定）
5. 解除は `tote.exe --uninstall`（exeを移動したら再登録）

## ビューアの操作
- ダブルクリック: フォルダへ入る / ファイルは一時フォルダへ展開して開く（実行形式は確認あり）
- 項目を **Explorer / デスクトップへドラッグ** → その項目だけ展開される
- Ctrl/Shift クリックで複数選択、Backspace で一つ上へ、列見出しで並べ替え
- 「すべて展開」→ ZIP名のフォルダへ / 「選択を展開…」→ フォルダを選んで展開
- 警告表示: 実行形式(exe/ps1/lnk 等)、不正パス(`..` や絶対パス＝展開されない)、パスワード付き(未対応・スキップ)
- 既存ファイルは上書きしない（`name (1)`）、更新日時は保持

## テスト
    cargo test --workspace --no-default-features   # Linux でも可（ビューアUI抜き）
    node --test app/tests-ui/tree.test.js

## 構成
- core/  ZIP作成・一覧・展開ロジック
- app/   exe（引数処理、集約、レジストリ登録）、app/src/viewer.rs（Tauri）、app/ui/（画面）

## 今後
v0.3: ZIPへのドロップ追加 / パスワード付きZIP / 他形式
