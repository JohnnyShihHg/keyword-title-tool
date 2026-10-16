#!/usr/bin/env bash
# 發布新版：改版本號 → 跑測試 → commit → 打標籤 → 推上 GitHub（CI 會自動打包發布）。
#
#   scripts/release.sh 0.1.1 "修正某某問題"
set -euo pipefail

VERSION="${1:-}"
NOTE="${2:-}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "用法：scripts/release.sh <版本號，例如 0.1.1> [更新說明]" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

if [[ -n "$(git status --porcelain)" ]]; then
  echo "有尚未 commit 的修改，請先 commit 或還原再發布。" >&2
  git status --short >&2
  exit 1
fi
if git rev-parse "v$VERSION" >/dev/null 2>&1; then
  echo "標籤 v$VERSION 已經存在。" >&2
  exit 1
fi

# tauri.conf.json 的 version 決定安裝檔與自動更新比對的版本；Cargo.toml 跟著改保持一致。
python3 - "$VERSION" <<'PY'
import json, re, sys
v = sys.argv[1]
p = "app/src-tauri/tauri.conf.json"
c = json.load(open(p, encoding="utf-8"))
c["version"] = v
with open(p, "w", encoding="utf-8") as f:
    json.dump(c, f, ensure_ascii=False, indent=2)
    f.write("\n")
p = "app/src-tauri/Cargo.toml"
s = open(p, encoding="utf-8").read()
s = re.sub(r'(?m)^version = "[^"]+"', f'version = "{v}"', s, count=1)
open(p, "w", encoding="utf-8").write(s)
PY

cargo test -p keyword-title-core --quiet
cargo build -p keyword-title-tool --quiet   # 順便更新 Cargo.lock

git add app/src-tauri/tauri.conf.json app/src-tauri/Cargo.toml Cargo.lock
git commit -m "版本 $VERSION${NOTE:+：$NOTE}"
git tag "v$VERSION"
git push origin HEAD "v$VERSION"

echo
echo "已推上 v${VERSION}，GitHub Actions 打包約需 10～15 分鐘："
echo "  https://github.com/JohnnyShihHg/keyword-title-tool/actions"
