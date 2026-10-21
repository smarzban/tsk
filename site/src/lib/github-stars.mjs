// The repository's star count, read once per build. A failed or slow request
// yields null and the count stays hidden until the browser refreshes it.
export const STARS_API = 'https://api.github.com/repos/smarzban/tsk';

let pending;

export function repoStars() {
  pending ??= fetchStars();
  return pending;
}

async function fetchStars() {
  try {
    const headers = { Accept: 'application/vnd.github+json' };
    if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
    const response = await fetch(STARS_API, { headers, signal: AbortSignal.timeout(4000) });
    if (!response.ok) return null;
    const count = (await response.json()).stargazers_count;
    return Number.isInteger(count) ? count : null;
  } catch {
    return null;
  }
}

// 223, 1.2k, 12k: the same rule as public/github-stars.js.
export function formatStars(count) {
  if (count < 1000) return String(count);
  const thousands = count / 1000;
  return `${thousands < 10 ? Math.floor(thousands * 10) / 10 : Math.floor(thousands)}k`;
}
