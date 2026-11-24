import { chromium } from "@playwright/test";
import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const outputRoot = path.join(root, ".audit", "visual-parity");
const baselineDir = path.join(outputRoot, "baseline");
const currentDir = path.join(outputRoot, "current");
const diffDir = path.join(outputRoot, "diff");
const prototype = args.find((arg) => !arg.startsWith("--")) ?? "C:\\Projects\\relay-proto-vi\\src\\hover\\visual-identity.prototype.html";
const refreshBaseline = args.includes("--refresh-baseline");
const production = args.includes("--production");
const viewport = { width: 1280, height: 860 };
const fixedTime = new Date("2026-11-21T12:00:00");
const draft = [
  "CSV export hangs past 30s for big accounts, see @exp",
  "! stream the export endpoint @web",
  "? check whether @CU-8x1k2 is the same bug",
  "?? should big exports move to a job queue",
].join("\n");

const states = [
  { name: "shell", target: null, app: null },
  { name: "rail", target: 'section[data-v="J"] .side', app: '[data-parity="rail"]' },
  { name: "timeline", target: 'section[data-v="J"] .tl', app: '[data-parity="timeline"]' },
  { name: "composer-empty", target: 'section[data-v="J"] .spot', app: '[data-parity="composer"]' },
  { name: "composer-draft", target: 'section[data-v="J"] .spot', app: '[data-parity="composer"]' },
  { name: "composer-picker", target: 'section[data-v="J"] .spot', app: '[data-parity="composer"], .ref-picker' },
];

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const hashFile = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");

async function waitForServer(url) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {}
    await sleep(200);
  }
  throw new Error(`Vite did not start at ${url}`);
}

async function settle(page) {
  await page.evaluate(async () => {
    await Promise.all([
      document.fonts.load('400 20px "Bricolage Grotesque"'),
      document.fonts.load('700 20px "Bricolage Grotesque"'),
      document.fonts.load('800 26px "Bricolage Grotesque"'),
      document.fonts.load('400 11px "Geist Mono"'),
      document.fonts.load('500 11px "Geist Mono"'),
      document.fonts.load('700 13px "Geist Mono"'),
    ]);
    await document.fonts.ready;
  });
  await sleep(250);
}

async function warmApp(page) {
  await page.reload({ waitUntil: "networkidle" });
}

async function setDraft(page, surface, text) {
  if (surface === "target") {
    await page.evaluate((value) => {
      const input = document.querySelector('section[data-v="J"] textarea');
      input.value = value;
      input.selectionStart = input.selectionEnd = value.length;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }, text);
  } else {
    const input = page.locator('[data-parity="composer"] textarea');
    await input.fill(text);
    await input.evaluate((element) => {
      element.selectionStart = element.selectionEnd = element.value.length;
    });
  }
  await settle(page);
}

async function captureElement(page, selector, file) {
  const element = page.locator(selector).first();
  await element.waitFor({ state: "visible" });
  await element.screenshot({ path: file, animations: "disabled" });
}

async function captureUnion(page, selectors, file) {
  const boxes = await Promise.all(
    selectors.map(async (selector) => {
      const element = page.locator(selector).first();
      await element.waitFor({ state: "attached" });
      const box = await element.boundingBox();
      if (!box) throw new Error(`No box for ${selector}`);
      return box;
    }),
  );
  const left = Math.min(...boxes.map((box) => box.x));
  const top = Math.min(...boxes.map((box) => box.y));
  const right = Math.max(...boxes.map((box) => box.x + box.width));
  const bottom = Math.max(...boxes.map((box) => box.y + box.height));
  await page.screenshot({
    path: file,
    clip: { x: left, y: top, width: right - left, height: bottom - top },
    animations: "disabled",
  });
}

async function captureSurface(page, surface, directory) {
  for (const state of states) {
    const file = path.join(directory, `${state.name}.png`);
    if (state.name === "shell") {
      await page.screenshot({ path: file, fullPage: true, animations: "disabled" });
      continue;
    }
    if (state.name === "composer-empty") {
      await setDraft(page, surface, "");
      await captureElement(page, state[surface], file);
      continue;
    }
    if (state.name === "composer-draft") {
      await setDraft(page, surface, draft);
      await captureElement(page, state[surface], file);
      continue;
    }
    if (state.name === "composer-picker") {
      await setDraft(page, surface, "@billing");
      if (surface === "target") {
        await page.locator('section[data-v="J"] .pick.open').waitFor({ state: "visible" });
      } else {
        await page.locator('.ref-picker[data-open="true"]').waitFor({ state: "visible" });
      }
      const selectors = surface === "target"
        ? [state.target]
        : ['[data-parity="composer"]', ".ref-picker"];
      await captureUnion(page, selectors, file);
      continue;
    }
    await captureElement(page, state[surface], file);
  }
}

function runDiff(target, current, output, selfTest = false) {
  const args = [
    path.join(root, "scripts", "visual-diff.py"),
    target,
    current,
    output,
  ];
  if (selfTest) args.push("--self-test");
  const result = spawnSync("python", args, { cwd: root, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(result.stderr || result.stdout || `visual-diff failed for ${current}`);
  }
  return JSON.parse(result.stdout.trim());
}

mkdirSync(outputRoot, { recursive: true });
rmSync(currentDir, { recursive: true, force: true });
rmSync(diffDir, { recursive: true, force: true });
mkdirSync(currentDir, { recursive: true });
mkdirSync(diffDir, { recursive: true });

const sourceHash = hashFile(prototype);
const hashFileSaved = path.join(baselineDir, "source.sha256");
const baselineFiles = states.map((state) => path.join(baselineDir, `${state.name}.png`));
const baselineExists = baselineFiles.every((file) => existsSync(file));
const partialBaseline = baselineFiles.some((file) => existsSync(file)) && !baselineExists;

if (partialBaseline && !refreshBaseline) {
  throw new Error("Partial visual baseline found. Remove .audit/visual-parity/baseline or pass --refresh-baseline.");
}
if (baselineExists && !refreshBaseline) {
  const savedHash = existsSync(hashFileSaved) ? readFileSync(hashFileSaved, "utf8").trim() : null;
  if (savedHash !== sourceHash) {
    throw new Error("Prototype changed after the baseline was captured. Review the change, then pass --refresh-baseline explicitly.");
  }
}
if (refreshBaseline) rmSync(baselineDir, { recursive: true, force: true });
mkdirSync(baselineDir, { recursive: true });

if (production && !existsSync(path.join(root, "dist", "index.html"))) {
  throw new Error("Production build missing. Run pnpm build first.");
}

const server = spawn("pnpm", production
  ? ["exec", "vite", "preview", "--host", "127.0.0.1", "--port", "4173", "--strictPort"]
  : ["exec", "vite", "--host", "127.0.0.1", "--port", "4173", "--strictPort"], {
  cwd: root,
  stdio: ["ignore", "pipe", "pipe"],
  windowsHide: true,
});
let serverOutput = "";
server.stdout.on("data", (chunk) => { serverOutput += chunk.toString(); });
server.stderr.on("data", (chunk) => { serverOutput += chunk.toString(); });

let browser;
try {
  await waitForServer("http://127.0.0.1:4173");
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport,
    colorScheme: "dark",
    reducedMotion: "reduce",
    deviceScaleFactor: 1,
  });

  if (!baselineExists || refreshBaseline) {
    const targetPage = await context.newPage();
    await targetPage.clock.install({ time: fixedTime });
    await targetPage.goto(`${pathToFileURL(prototype).href}?variant=J`, { waitUntil: "networkidle" });
    await targetPage.addStyleTag({ content: "#sw { display: none !important; }" });
    await targetPage.waitForSelector('section[data-v="J"] .spot');
    await settle(targetPage);
    await captureSurface(targetPage, "target", baselineDir);
    writeFileSync(hashFileSaved, `${sourceHash}\n`);
    await targetPage.close();
  }

  const appPage = await context.newPage();
  await appPage.clock.install({ time: fixedTime });
  await appPage.goto("http://127.0.0.1:4173/?visual-fixture=J", { waitUntil: "networkidle" });
  await warmApp(appPage);
  await appPage.waitForSelector('[data-parity="timeline"]');
  await settle(appPage);
  await captureSurface(appPage, "app", currentDir);

  const hoverPage = await context.newPage();
  await hoverPage.clock.install({ time: fixedTime });
  const hoverErrors = [];
  hoverPage.on("pageerror", (error) => hoverErrors.push(error.message));
  await hoverPage.goto("http://127.0.0.1:4173/hover.html?visual-fixture=J", { waitUntil: "networkidle" });
  await warmApp(hoverPage);
  await hoverPage.waitForSelector(".dock-content textarea");
  const hoverInput = hoverPage.locator(".dock-content textarea");
  await hoverInput.fill(draft);
  const draftLines = await hoverPage.locator(".dock-content .de-line").count();
  await hoverInput.fill("@billing");
  await hoverPage.waitForSelector('.dock-content .ref-picker[data-open="true"]');
  const lensVisible = await hoverPage.locator(".dock-content .ref-lens").isVisible();
  const hoverSmoke = {
    pass: hoverErrors.length === 0 && draftLines === 4 && lensVisible,
    draftLines,
    lensVisible,
    errors: hoverErrors,
  };
  await hoverPage.close();

  const selfTest = runDiff(
    path.join(baselineDir, "shell.png"),
    path.join(baselineDir, "shell.png"),
    path.join(diffDir, "self-test.png"),
    true,
  );
  const results = states.map((state) => {
    const diff = runDiff(
      path.join(baselineDir, `${state.name}.png`),
      path.join(currentDir, `${state.name}.png`),
      path.join(diffDir, `${state.name}.png`),
    );
    return { name: state.name, ...diff };
  });
  const report = {
    prototype,
    sourceHash,
    viewport,
    fixedTime: fixedTime.toISOString(),
    baselineCreated: !baselineExists || refreshBaseline,
    production,
    selfTest,
    hoverSmoke,
    results,
    pass: hoverSmoke.pass && results.every((result) => result.pass),
  };
  writeFileSync(path.join(outputRoot, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`Prototype SHA-256: ${sourceHash}`);
  console.log(`Baseline: ${report.baselineCreated ? "created" : "reused (immutable)"}`);
  console.log(`Surface: ${production ? "production build" : "Vite dev"}`);
  console.log(`Self-test: PASS (${selfTest.changedPixels} changed pixel detected)`);
  console.log(`Hover smoke: ${hoverSmoke.pass ? "PASS" : "FAIL"} (${hoverSmoke.draftLines} draft lines, lens ${hoverSmoke.lensVisible ? "visible" : "hidden"})`);
  for (const result of results) {
    const size = `${result.currentSize[0]}x${result.currentSize[1]}`;
    const pct = (result.changedRatio * 100).toFixed(2);
    console.log(`${result.pass ? "PASS" : "FAIL"} ${result.name.padEnd(16)} ${size.padStart(9)} ${pct.padStart(6)}% changed`);
  }
  console.log(`Report: ${path.join(outputRoot, "report.json")}`);
  if (!report.pass) process.exitCode = 1;
} catch (error) {
  console.error(error);
  if (serverOutput) console.error(serverOutput);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (server.pid) {
    if (process.platform === "win32") {
      spawnSync("taskkill", ["/pid", String(server.pid), "/T", "/F"], { stdio: "ignore" });
    } else {
      server.kill("SIGTERM");
    }
  }
}
