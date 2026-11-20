#!/usr/bin/env bash
# 發布新版：檢查遠端 → 改版本號 → 跑 check → commit → 打標籤 → 推上 GitHub（CI 會再跑一次 check 才打包發布）。
# 只在使用者說「發版」時才跑。程式的修改要先 commit 好（未追蹤的檔案沒關係，例如自己的筆記）。
#
#   scripts/release.sh 0.6.39 "修正：…；改善：…"
set -euo pipefail

VERSION="${1:-}"
NOTE="${2:-}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "用法：scripts/release.sh <版本號，例如 0.6.39> [更新說明]" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  echo "有尚未 commit 的修改，請先 commit 或還原再發布：" >&2
  git status --short --untracked-files=no >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null; then
  echo "標籤 v$VERSION 已經存在。" >&2
  exit 1
fi

# 別台電腦可能發過版：遠端比較新就停下，先合併
git fetch --tags --quiet origin
if [[ -n "$(git log --oneline HEAD..origin/main)" ]]; then
  echo "遠端 main 有這台沒有的 commit，先 git merge --ff-only origin/main 再發布：" >&2
  git log --oneline HEAD..origin/main >&2
  exit 1
fi

# 不要內網位址進公開 repo（AGENTS.md）
if git grep -nE '\b10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b|\b192\.168\.[0-9]{1,3}\.[0-9]{1,3}\b' -- . ':!Cargo.lock' ':!package-lock.json'; then
  echo "上面這些地方有內網位址，repo 是公開的，請先遮掉。" >&2
  exit 1
fi

# tauri.conf.json 的 version 決定安裝檔與自動更新比對的版本；Cargo.toml 跟著改。
# 用 sed 只換那一行（不會改格式、不會加 BOM）。
sed -i -E "0,/\"version\": \"[^\"]+\"/s//\"version\": \"$VERSION\"/" app/src-tauri/tauri.conf.json
sed -i -E "0,/^version = \"[^\"]+\"/s//version = \"$VERSION\"/" app/src-tauri/Cargo.toml
grep -q "\"version\": \"$VERSION\"" app/src-tauri/tauri.conf.json
grep -q "^version = \"$VERSION\"" app/src-tauri/Cargo.toml

# 測試＋clippy＋JS 檢查（也會順便更新 Cargo.lock 的版本號）
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/check.ps1

git add app/src-tauri/tauri.conf.json app/src-tauri/Cargo.toml Cargo.lock
git commit -q -F - <<EOF
版本 $VERSION${NOTE:+：$NOTE}

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
git tag "v$VERSION"
git push origin HEAD "v$VERSION"

echo
echo "已推上 v${VERSION}，GitHub Actions 檢查＋打包約需 10～15 分鐘："
echo "  https://github.com/JohnnyShihHg/keyword-title-tool/actions"
