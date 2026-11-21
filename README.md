# web-blocker (animeblock)

アニメを中心とした動画配信サイトへのアクセスをブロックする Windows 用の常駐サービスです。
サイトは 2 種類に分かれます。

- **恒久ブロック**: 時間に関係なく常にブロック。一度入れたら外せません
- **時間帯ブロック**: 毎日決まった時間帯 (既定 21:00〜07:00) だけブロック。時間外なら外せます

**アンインストールの手段はありません。** 恒久ブロックを外させないため、意図的に用意していません。

## 仕組み

| 層 | 内容 |
|---|---|
| hosts ファイル | 対象ドメインを 0.0.0.0 / ::1 に向ける。全ブラウザ・アプリに効く。ブロック対象がある間 (恒久ブロックがあるので実質常時) はファイルを開きっぱなしにして他から書き換え・削除できなくし、管理区間を消されても 2 秒以内に戻す |
| ブラウザポリシー | Chrome / Edge / Brave の URLBlocklist、Firefox の WebsiteFilter に書き込む。サブドメインやパス (例: `amazon.co.jp/gp/video`) まで止められる |
| 暗号化 DNS の無効化 | ブラウザの DNS over HTTPS をポリシーで止め、hosts を迂回されないようにする |
| 時刻 | 時刻サーバー (NTP) と同期し、OS の時計やタイムゾーンを変えても判定がずれない |
| サービス保護 | SYSTEM 権限で動き、管理者でも停止・設定変更・削除できない。強制終了されても 1 秒後に再起動し、セーフモードでも起動する |

## ビルド

```
cargo build --release
```

`target\release\animeblock.exe` ができます。

## 使い方

管理者としてターミナルを開いて実行します (`status` と `list` は一般ユーザーでも可)。

```
animeblock install                   # インストールしてすぐ開始
animeblock status                    # 状態確認
animeblock list                      # 対象サイト一覧
animeblock add example.com           # 時間帯ブロックに追加 (いつでも可)
animeblock add --permanent example.com  # 恒久ブロックに追加 (いつでも可。二度と外せない)
animeblock remove example.com        # 時間帯ブロックから外す (ブロック時間外のみ。恒久ブロックは外せない)
animeblock set-hours 22:00 06:00     # 時間帯ブロックの時間変更 (ブロック時間外のみ)
```

インストール後の実行ファイルは `C:\Program Files\AnimeBlock\animeblock.exe` です。

**注意: インストールした時点から恒久ブロックが始まり、二度と解除できません。新しい版への入れ替えもできません。**

### 動作確認 (システムを変更しない)

```
animeblock simulate --hosts .\test-hosts --at 21:00
```

指定したファイルを hosts に見立てて 1 回だけ処理します。ポリシーやサービスには触れません。

## 既定のブロック対象

- 恒久ブロック: dアニメストア、Lemino、ABEMA、Netflix、Prime Video、U-NEXT、DMM TV、Disney+、Hulu、
  FOD、TVer、TELASA、バンダイチャンネル、ニコニコ、Crunchyroll、HIDIVE、bilibili
- 時間帯ブロック: YouTube

一覧は `src/sites.rs` または `animeblock list` で確認できます。

## 防げないこと

管理者権限を持つ本人が時間と手間をかければ突破できます。主な抜け道:

- フォルダや設定ファイルの所有権を奪って書き換え、サービスを強制終了して再起動させる
- 別の OS や USB ブートで起動して hosts を書き換える
- ポリシーに対応していないブラウザ・アプリで、独自の DNS や VPN・プロキシを使う
- 時刻サーバーへの通信を遮断したうえで、PC 起動前に BIOS の時計を進める
- スマホなど別の端末を使う

また、インストール中は他のソフトも hosts ファイルを書き換えられません。
