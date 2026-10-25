# iBPS

IPS、アドレス拡張版IPS、UPS、BPS形式に対応したWindows用のパッチ作成・適用ツール。

## 動作環境

* Windows 10 以降（64bit）

## ダウンロード

[ibps.exeをダウンロード](https://github.com/608/iBPS/releases/latest/download/ibps.exe)

## 使い方

1. `ibps.exe` を起動
2. 「パッチ適用」または「パッチ作成」を選択
3. ファイルを指定して「実行」をクリック

## CLI実行

### パッチ適用

```sh
ibps.exe --apply <patch> <input> <output>

```

### パッチ作成

```sh
ibps.exe --create <--ips|--ips32|--ups|--bps> <original> <modified> <output>

```
