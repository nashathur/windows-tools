#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { pathToFileURL, fileURLToPath } = require('url');

// Printable A4 width in CSS px: 210 mm page minus two 20 mm margins (see md2pdf.css).
const PRINT_WIDTH = Math.floor((210 - 40) / 25.4 * 96);

function validateTex(katex, tex, display) {
  try {
    katex.renderToString(tex, { displayMode: !!display, throwOnError: true, strict: 'ignore' });
    return null;
  } catch (error) {
    return String(error.message || error).replace(/^KaTeX parse error:\s*/, '');
  }
}

function readMarkdown(file) {
  return fs.readFileSync(file, 'utf8').replace(/^﻿/, '');
}

// Math is a marked extension, so code spans, fenced and indented code (also inside lists) are
// tokenized by marked first and never parsed for math. `\$` is an escape token, so it stays literal.
function mathExtensions(katex, state) {
  const render = (tex, displayMode) => {
    const options = { displayMode, strict: 'ignore' };
    const error = validateTex(katex, tex, displayMode);
    if (error === null) return katex.renderToString(tex, { ...options, throwOnError: true });
    state.errors += 1;
    return katex.renderToString(tex, { ...options, throwOnError: false });
  };
  return {
    extensions: [
      {
        name: 'displayMathBlock', level: 'block',
        start: src => { const i = src.indexOf('$$'); return i < 0 ? undefined : i; },
        tokenizer(src) {
          const m = /^ {0,3}\$\$([\s\S]+?)\$\$[ \t]*(?:\n|$)/.exec(src);
          if (m) return { type: 'displayMathBlock', raw: m[0], tex: m[1] };
        },
        renderer: token => render(token.tex, true) + '\n',
      },
      {
        name: 'displayMath', level: 'inline',
        start: src => { const i = src.indexOf('$$'); return i < 0 ? undefined : i; },
        tokenizer(src) {
          const m = /^\$\$([\s\S]+?)\$\$/.exec(src);
          if (m) return { type: 'displayMath', raw: m[0], tex: m[1] };
        },
        renderer: token => render(token.tex, true),
      },
      {
        name: 'inlineMath', level: 'inline',
        start: src => { const i = src.indexOf('$'); return i < 0 ? undefined : i; },
        tokenizer(src) {
          // `\$` inside the formula is an escaped dollar, never the closing one.
          const m = /^\$((?:\\.|[^\s$\\])(?:(?:\\.|[^$\\])*?(?:\\.|[^\s$\\]))?)\$(?!\d)/s.exec(src);
          if (m) return { type: 'inlineMath', raw: m[0], tex: m[1] };
        },
        renderer: token => render(token.tex, false),
      },
    ],
  };
}

function configure(marked, katex, state) {
  marked.use(mathExtensions(katex, state));
  return marked;
}

function convert(markdown, marked, katex) {
  const state = { errors: 0 };
  configure(marked, katex, state);
  const html = marked.parse(markdown, { gfm: true, breaks: false });
  return { html, errors: state.errors };
}

// Named entities come from a table the caller supplies (--tokens FILE ENTITIES.json, generated from
// Python's complete HTML5 table); numeric references and the basic five are always decoded.
let namedEntities = {};
function unescapeHtml(text) {
  return String(text).replace(/&(#x[0-9a-f]+|#\d+|[a-z][a-z0-9]*);/gi, (all, name) => {
    if (name[0] === '#') {
      const code = name[1].toLowerCase() === 'x' ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
      try { return String.fromCodePoint(code); } catch (_) { return all; }
    }
    const basic = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" }[name];
    if (basic) return basic;
    return Object.prototype.hasOwnProperty.call(namedEntities, name) ? namedEntities[name] : all;
  });
}

function htmlVisibleText(source) {
  return unescapeHtml(String(source).replace(/<!--[\s\S]*?-->/g, ' ')
    .replace(/<(script|style|noscript)\b[^>]*>[\s\S]*?<\/\1\s*>/gi, ' ')
    .replace(/<[^>]*>/g, ' '));
}

// Visible text of inline tokens: link text, emphasis content and code spans are kept (code spans
// are also listed in `code`); images and math produce no words (images count separately).
function inlineText(tokens, out) {
  for (const t of tokens || []) {
    switch (t.type) {
      case 'text': case 'escape':
        if (t.tokens && t.tokens.length) inlineText(t.tokens, out); else out.text.push(unescapeHtml(t.text));
        break;
      case 'codespan': { const code = unescapeHtml(t.text); out.text.push(code); out.code.push(code); break; }
      case 'strong': case 'em': case 'del': case 'link': inlineText(t.tokens, out); break;
      case 'br': out.text.push(' '); break;
      case 'html': out.text.push(htmlVisibleText(t.text)); break;
      case 'image': out.images += 1; break;
      default: break; // math, inline html
    }
  }
}

function blocks(markdown, marked, katex) {
  configure(marked, katex, { errors: 0 });
  const tokens = marked.lexer(markdown, { ...marked.defaults, gfm: true });
  const result = []; let section = 'document'; const counters = { images: 0 };
  function emit(type, inline, fallback) {
    const out = { text: [], code: [], images: 0 };
    if (inline && inline.length) inlineText(inline, out); else if (fallback) out.text.push(unescapeHtml(fallback));
    counters.images += out.images;
    const text = out.text.join('').replace(/\s+/g, ' ').trim();
    if (text) result.push({ type, section, text, code: out.code });
    return text;
  }
  function walk(token) {
    switch (token.type) {
      case 'heading': { const text = emit('heading', token.tokens, token.text); if (text) section = text; break; }
      case 'paragraph': emit('paragraph', token.tokens, token.text); break;
      case 'text': emit('paragraph', token.tokens, token.text); break;
      case 'html': emit('paragraph', null, htmlVisibleText(token.text)); break;
      case 'code': if (token.text && token.text.trim()) result.push({ type: 'code', section, text: token.text, code: [token.text] }); break;
      case 'list': for (const item of token.items || []) for (const child of item.tokens || []) {
        if (child.type === 'text' || child.type === 'paragraph') emit('list_item', child.tokens, child.text); else walk(child);
      } break;
      case 'table':
        for (const cell of token.header || []) emit('table_cell', cell.tokens, cell.text);
        for (const row of token.rows || []) for (const cell of row) emit('table_cell', cell.tokens, cell.text);
        break;
      case 'blockquote': for (const child of token.tokens || []) walk(child); break;
      default: break;
    }
  }
  for (const t of tokens) walk(t);
  return { blocks: result, images: counters.images };
}

// Canonical path (junctions and symlinks resolved; lower-cased on Windows). A path that does not
// exist yet is resolved through its nearest existing parent, so a missing file under a junction
// is still judged by where the junction points.
function canonical(p) {
  const fold = r => (process.platform === 'win32' ? r.toLowerCase() : r);
  const resolve = q => {
    try { return fs.realpathSync.native(q); } catch (_) {
      const parent = path.dirname(q);
      if (parent === q) throw new Error('unresolvable path');
      return path.join(resolve(parent), path.basename(q));
    }
  };
  return fold(resolve(path.resolve(p)));
}

function rejectRemoteReparseTargets(p) {
  const absolute = path.resolve(p);
  const remote = value => /^(?:\\\\\?\\UNC\\|\\\\[^\\]+\\[^\\]+|\/\/[^/]+\/[^/]+)/i.test(value);
  if (remote(absolute)) throw new Error('remote path is not allowed');
  const pending = [absolute], checked = new Set();
  while (pending.length) {
    const candidate = path.resolve(pending.pop());
    if (remote(candidate)) throw new Error('remote reparse target is not allowed');
    if (checked.has(candidate)) continue;
    checked.add(candidate);
    const parsed = path.parse(candidate);
    let current = parsed.root;
    for (const part of candidate.slice(parsed.root.length).split(path.sep).filter(Boolean)) {
      current = path.join(current, part);
      let stat;
      try { stat = fs.lstatSync(current); } catch (_) { continue; }
      if (!stat.isSymbolicLink()) continue;
      const target = fs.readlinkSync(current);
      const resolved = path.resolve(path.dirname(current), target);
      if (remote(target) || remote(resolved)) throw new Error('remote reparse target is not allowed');
      pending.push(resolved);
    }
  }
}

function inside(file, root) {
  return file === root || file.startsWith(root.endsWith(path.sep) ? root : root + path.sep);
}

async function render(source, output, marked, katex) {
  const puppeteer = require('puppeteer-core');
  const edge = process.env.PDFTOOLS_EDGE;
  const htmlPath = path.resolve(process.env.PDFTOOLS_HTML);
  const cssPath = path.resolve(process.env.PDFTOOLS_CSS);
  const sourcePath = path.resolve(source);
  const folder = path.dirname(sourcePath);
  const markdown = readMarkdown(sourcePath);
  const converted = convert(markdown, marked, katex);
  const attackMarker = crypto.randomBytes(24).toString('hex');
  const katexRoot = path.resolve(require.resolve('katex/package.json'), '..');
  const fontsUrl = pathToFileURL(path.join(katexRoot, 'dist', 'fonts') + path.sep).href;
  const katexCss = fs.readFileSync(path.join(katexRoot, 'dist', 'katex.min.css'), 'utf8').replace(/url\(fonts\//g, `url(${fontsUrl}`);
  const css = katexCss + '\n' + fs.readFileSync(cssPath, 'utf8');
  const csp = "default-src 'none'; img-src file:; style-src 'unsafe-inline' file:; font-src file:; script-src 'none'";
  // A meta refresh would navigate the page (aborting that request still commits an error page and
  // destroys the render context), so raw <meta> tags from the Markdown never reach the browser.
  let strippedMeta = false;
  converted.html = converted.html.replace(/<meta\b[^>]*>/gi, () => { strippedMeta = true; return ''; });
  const documentHtml = `<!doctype html><html><head><meta charset="utf-8"><base href="${pathToFileURL(folder + path.sep).href}"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>${css}</style></head><body>${converted.html}</body></html>`;
  fs.writeFileSync(htmlPath, documentHtml, 'utf8');
  rejectRemoteReparseTargets(sourcePath);
  rejectRemoteReparseTargets(htmlPath);
  const realFolder = canonical(folder), realKatex = canonical(katexRoot), realHtml = canonical(htmlPath);
  let remote = false, blockedLocal = strippedMeta;
  const browser = await puppeteer.launch({ executablePath: edge, headless: true, args: ['--disable-gpu'] });
  try {
    const page = await browser.newPage();
    await page.setRequestInterception(true);
    let initialNavigationSeen = false;
    page.on('request', request => {
      try {
        if (request.isNavigationRequest() && request.frame() === page.mainFrame()) {
          const requestUrl = new URL(request.url());
          if (requestUrl.protocol === 'file:' && !requestUrl.host)
            rejectRemoteReparseTargets(fileURLToPath(requestUrl));
          const requestedFile = requestUrl.protocol === 'file:' && !requestUrl.host
            ? canonical(fileURLToPath(requestUrl)) : null;
          if (!initialNavigationSeen && requestedFile === realHtml) {
            initialNavigationSeen = true;
          } else {
            if (requestUrl.protocol !== 'file:' || requestUrl.host) remote = true;
            else blockedLocal = true;
            return request.abort();
          }
        }
        const url = new URL(request.url());
        if (url.protocol !== 'file:' || url.host) { remote = true; return request.abort(); }
        rejectRemoteReparseTargets(fileURLToPath(url));
        const file = canonical(fileURLToPath(url));
        if (file === realHtml || inside(file, realFolder) || inside(file, realKatex)) return request.continue();
        blockedLocal = true;
      } catch (_) { blockedLocal = true; }
      request.abort();
    });
    // Loads the content security policy refuses never reach interception; record them here.
    await page.evaluateOnNewDocument(marker => {
      Object.defineProperty(window, '__p6ExpectedAttackMarker', { value: marker, configurable: false });
      window.__p6violations = [];
      document.addEventListener('securitypolicyviolation', e => window.__p6violations.push(String(e.blockedURI)));
    }, attackMarker);
    await page.emulateMediaType('print');
    await page.setViewport({ width: PRINT_WIDTH, height: 1123, deviceScaleFactor: 1 });
    await page.goto(pathToFileURL(htmlPath).href, { waitUntil: 'load', timeout: 60000 });
    const status = await page.evaluate(async () => {
      await Promise.race([document.fonts.ready, new Promise((_, reject) => setTimeout(() => reject(new Error('font timeout')), 60000))]);
      const images = [...document.images];
      await Promise.race([Promise.all(images.map(img => img.complete ? Promise.resolve() : new Promise(resolve => { img.onload = resolve; img.onerror = resolve; }))), new Promise((_, reject) => setTimeout(() => reject(new Error('image timeout')), 60000))]);
      const failed = images.filter(img => !img.naturalWidth && new URL(img.src, document.baseURI).protocol === 'file:').length;
      // A blocked or missing image would print as a broken-image icon (an image the source never had).
      for (const img of images) if (!img.naturalWidth) {
        const note = document.createElement('span');
        note.textContent = `[${img.alt || 'image'} not shown]`;
        img.replaceWith(note);
      }
      const avail = document.body.clientWidth;
      const equations = [...document.querySelectorAll('.katex-display')].map(el => {
        const width = el.scrollWidth;
        if (width <= avail) return 1;
        const ratio = avail / width, height = el.offsetHeight;
        el.style.textAlign = 'left';
        el.style.transformOrigin = 'left top';
        el.style.transform = `scale(${ratio})`;
        // The transform does not shrink the layout box: reclaim the space it no longer fills.
        el.style.marginBottom = `${parseFloat(getComputedStyle(el).marginBottom) - height * (1 - ratio)}px`;
        return ratio;
      });
      return { failed, equations, katexErrors: document.querySelectorAll('.katex-error').length,
        violations: window.__p6violations || [], attack: window.__p6AttackMarker === window.__p6ExpectedAttackMarker };
    });
    for (const blocked of status.violations) {
      if (blocked === 'inline' || blocked === 'eval') continue; // scripts: the policy doing its job
      if (blocked === 'data' || /^file:\/\/\/[^/]/i.test(blocked) || /^file:\/[^/]/i.test(blocked)) blockedLocal = true;
      else remote = true;
    }
    await page.pdf({ path: output, format: 'A4', preferCSSPageSize: true, printBackground: true });
    const warnings = [];
    if (remote) warnings.push('REMOTE_RESOURCE');
    if (blockedLocal) warnings.push('BLOCKED_LOCAL');
    if (status.failed) warnings.push('LOCAL_IMAGE_FAILED');
    if (status.katexErrors || converted.errors) warnings.push('KATEX_ERROR');
    if (status.equations.some(x => x < .7)) warnings.push('EQUATION_SHRUNK');
    fs.writeFileSync(process.env.PDFTOOLS_STATUS, JSON.stringify({
      warnings, errors: converted.errors, remote, blockedLocal, scriptExecuted: Boolean(status.attack),
    }), 'utf8');
  } finally { await browser.close(); }
}

async function main() {
  const marked = require('marked'), katex = require('katex');
  if (process.argv[2] === '--validate') {
    const items = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
    const failures = [];
    for (const item of items) {
      const error = validateTex(katex, item.latex, item.display);
      if (error !== null) failures.push({ id: item.id, error });
    }
    process.stdout.write(JSON.stringify(failures));
    return;
  }
  if (process.argv[2] === '--tokens') {
    const source = process.argv[3];
    if (process.argv[4]) namedEntities = JSON.parse(fs.readFileSync(process.argv[4], 'utf8'));
    process.stdout.write(JSON.stringify(blocks(readMarkdown(source), marked, katex)));
    return;
  }
  if (process.argv[2] === '--render') return render(process.argv[3], process.argv[4], marked, katex);
  throw new Error('usage: md2pdf.js --tokens FILE | --render FILE OUTPUT | --validate FILE');
}

module.exports = { rejectRemoteReparseTargets, blocks, htmlVisibleText, validateTex };
if (require.main === module)
  main().catch(error => { process.stderr.write(`md2pdf: ${error.message}\n`); process.exitCode = 1; });
