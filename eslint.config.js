// JS 檢查：只擋這個專案真的出過事、或 AUDIT_REPORT.md 確認過的寫法。
// 前端是好幾個 <script> 共用全域變數（app.js 先載入），所以不開 no-undef／no-unused-vars。
const globals = require("globals");

module.exports = [
  {
    files: ["app/src/**/*.js"],
    languageOptions: {
      ecmaVersion: 2024,
      sourceType: "script",
      globals: { ...globals.browser },
    },
    rules: {
      // 語法層級的錯（寫錯會整個檔案不能跑）
      "no-dupe-keys": "error",
      "no-dupe-else-if": "error",
      "no-duplicate-case": "error",
      "no-unreachable": "error",
      "no-self-assign": "error",
      "no-cond-assign": "error",
      "no-constant-condition": ["error", { checkLoops: false }],
      "no-redeclare": "error",
      "no-func-assign": "error",
      // 吞掉錯誤：失敗了畫面上什麼都看不到（AUDIT #8、#9）
      "no-empty": ["error", { allowEmptyCatch: false }],
      "no-restricted-syntax": [
        "error",
        {
          // 2026-11-20 播放清單搜尋欄不能打英文、不能刪、不能貼：onkeydown 回傳 false＝取消按鍵
          selector:
            "AssignmentExpression[left.type='MemberExpression'][left.property.name=/^on[a-z]+$/] > ArrowFunctionExpression[body.type=/^(LogicalExpression|ConditionalExpression)$/]",
          message:
            "不要寫 el.onX = (e) => a && b：回傳 false 會取消瀏覽器的預設動作（吃掉按鍵、點擊）。請改用 el.addEventListener(\"x\", (e) => { if (a) b; })。",
        },
        {
          selector: "CallExpression[callee.property.name='catch'] > ArrowFunctionExpression[body.type='BlockStatement'][body.body.length=0]",
          message:
            "不要寫 .catch(() => {})：失敗了畫面上什麼都看不到。請把錯誤顯示出來（acMessage／ytMessage／console.error），真的可以忽略的要寫註解說明為什麼。",
        },
      ],
    },
  },
];
