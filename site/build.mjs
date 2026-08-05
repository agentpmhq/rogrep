import { cp, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const siteDir = dirname(fileURLToPath(import.meta.url));
const outputDir = resolve(siteDir, "../_site");
const apiBase = "https://api.github.com/repos/agentpmhq/rogrep";
const headers = { Accept: "application/vnd.github+json", "User-Agent": "rogrep-pages-build" };
if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;

async function github(path) {
  const response = await fetch(`${apiBase}${path}`, { headers });
  if (!response.ok) throw new Error(`GitHub ${response.status}: ${path}`);
  return response;
}

function compact(number) {
  if (number < 1000) return String(number);
  return `${(number / 1000).toFixed(1).replace(/\.0$/, "")}k`;
}

function relativeDate(value) {
  const days = Math.max(0, Math.floor((Date.now() - new Date(value).getTime()) / 86_400_000));
  if (days === 0) return "today";
  if (days === 1) return "1 day ago";
  if (days < 30) return `${days} days ago`;
  const months = Math.floor(days / 30);
  return months === 1 ? "1 month ago" : `${months} months ago`;
}

async function getStats() {
  const stats = {};
  try {
    const repo = await (await github("")).json();
    if (Number.isFinite(repo.stargazers_count)) stats.stars = compact(repo.stargazers_count);
    if (repo.pushed_at) stats.lastCommit = relativeDate(repo.pushed_at);
  } catch (error) {
    console.warn(error.message);
  }
  try {
    const contributors = await (await github("/contributors?per_page=100&anon=true")).json();
    if (contributors.length) stats.contributors = compact(contributors.length);
  } catch (error) {
    console.warn(error.message);
  }
  try {
    const response = await github("/commits?per_page=1");
    const commits = await response.json();
    const lastPage = response.headers.get("link")?.match(/[?&]page=(\d+)>; rel="last"/)?.[1];
    const count = lastPage ? Number(lastPage) : commits.length;
    if (count) stats.commits = compact(count);
  } catch (error) {
    console.warn(error.message);
  }
  return stats;
}

await mkdir(outputDir, { recursive: true });
await Promise.all([
  cp(resolve(siteDir, "styles.css"), resolve(outputDir, "styles.css")),
  cp(resolve(siteDir, "site.js"), resolve(outputDir, "site.js")),
  cp(resolve(siteDir, ".nojekyll"), resolve(outputDir, ".nojekyll")),
]);

const template = await readFile(resolve(siteDir, "index.html"), "utf8");
const stats = await getStats();
const html = template.replace("<!--ROGREP_REPO_STATS-->", JSON.stringify(stats).replaceAll("<", "\\u003c"));
await writeFile(resolve(outputDir, "index.html"), html);
console.log(`Built Rogrep Pages with ${Object.keys(stats).length} live repository figures.`);
