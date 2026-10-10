// Validate LaTeX snippets with KaTeX (the renderer used by Obsidian, VS Code, etc.).
// Input:  JSON file with [{ "id": "...", "latex": "...", "display": true|false }, ...]
// Output: JSON on stdout: [{ "id": "...", "error": "..." }, ...] for snippets that fail to render.
// Usage:  node validate_math.js math.json

const fs = require("fs");
const katex = require("katex");

const items = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const failures = [];

for (const item of items) {
  try {
    katex.renderToString(item.latex, {
      displayMode: !!item.display,
      throwOnError: true,
      strict: "ignore", // only real parse errors count, not style warnings
    });
  } catch (e) {
    failures.push({ id: item.id, error: String(e.message || e).replace(/^KaTeX parse error: /, "") });
  }
}

process.stdout.write(JSON.stringify(failures));
