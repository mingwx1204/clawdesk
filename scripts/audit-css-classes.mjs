#!/usr/bin/env node
/**
 * CSS 类名回归审计：扫描 Vue 模板中引用的 class，检查是否存在对应 CSS 定义。
 *
 * 设计目标：防止再次出现「删除某个 css 文件时误删仍被引用的样式类」这类回归
 * （例：game.css 删除后 .perm-overlay / .perm-card 仍被 App.vue 使用）。
 *
 * 用法：
 *   node scripts/audit-css-classes.mjs
 * 返回码：
 *   0 = 全部通过；1 = 发现缺失类（白名单外的）
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const ROOT = process.cwd();
const SRC = join(ROOT, "src");

/** 结构类/由父元素样式覆盖的类：确认无独立样式也属预期。 */
const WHITELIST = new Set([
  "root",
  "tb-title",
]);

/** 动态 :class 三元表达式里的状态字符串，不是类名，仅作为条件值出现。 */
const DYNAMIC_IGNORE = new Set(["success", "error", "danger", "running"]);

const missing = new Map(); // className -> Set("file:line")
const defined = new Set();

function walk(dir, exts, out = []) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    const st = statSync(full);
    if (st.isDirectory()) walk(full, exts, out);
    else if (exts.some((e) => full.endsWith(e))) out.push(full);
  }
  return out;
}

function addMissing(file, line, className) {
  if (!className || WHITELIST.has(className) || DYNAMIC_IGNORE.has(className) || defined.has(className)) return;
  if (!missing.has(className)) missing.set(className, new Set());
  missing.get(className).add(`${file}:${line}`);
}

function lineOf(text, index) {
  return text.slice(0, index).split("\n").length;
}

// 1) 收集所有 CSS 定义（独立 css 文件 + Vue <style> 块）
for (const file of walk(SRC, [".css", ".vue"])) {
  let css = "";
  if (file.endsWith(".css")) {
    css = readFileSync(file, "utf8");
  } else {
    const text = readFileSync(file, "utf8");
    for (const m of text.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)) css += "\n" + m[1];
  }
  for (const m of css.matchAll(/\.([A-Za-z_][\w-]*)/g)) defined.add(m[1]);
}

// 2) 收集 Vue 模板引用的 class（静态 class + :class 内的对象键/字符串字面量）
for (const file of walk(SRC, [".vue"])) {
  const text = readFileSync(file, "utf8");

  for (const m of text.matchAll(/\bclass="([^"]*)"/g)) {
    const raw = m[1];
    const line = lineOf(text, m.index);

    if (/^[\s]*\{/.test(raw)) {
      // :class="{ active: cond, 'is-open': cond }"：只审计对象键
      for (const key of raw.matchAll(/(?:^|[,\{]\s*)(?:['"]([\w-]+)['"]|([A-Za-z_][\w-]*))\s*:/g)) {
        addMissing(file, line, key[1] || key[2]);
      }
    } else if (/^[\s]*\[/.test(raw)) {
      // :class="['foo', cond && 'bar']"：数组里所有字符串字面量都是候选类名
      for (const str of raw.matchAll(/['"]([\w-]+)['"]/g)) addMissing(file, line, str[1]);
    } else if (/[?()'"]/.test(raw)) {
      // :class="cond ? 'a' : 'b'"：三元分支字符串是候选类名（条件值由 DYNAMIC_IGNORE 排除）
      for (const str of raw.matchAll(/['"]([\w-]+)['"]/g)) addMissing(file, line, str[1]);
    } else {
      // 普通静态 class="a b c"
      for (const cls of raw.trim().split(/\s+/)) addMissing(file, line, cls);
    }
  }
}

// 3) 输出报告
if (missing.size === 0) {
  console.log(`✅ CSS 类名审计通过：所有模板引用的 class 均有定义（白名单 ${WHITELIST.size} 个）。`);
  process.exit(0);
}

const lines = [];
for (const [cls, locs] of [...missing.entries()].sort()) {
  lines.push(`- .${cls} 未被定义，但被引用：`);
  for (const loc of [...locs].sort()) lines.push(`    ${relative(ROOT, loc)}`);
}
console.error(`❌ CSS 类名审计发现 ${missing.size} 个缺失类：`);
console.error(lines.join("\n"));
process.exit(1);
