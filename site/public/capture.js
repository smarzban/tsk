function normalizeThread(raw) {
  const input = String(raw);
  if (!/^[A-Za-z0-9][A-Za-z0-9.-]{0,31}$/.test(input)) return null;
  return input.toLowerCase();
}

export function parseCapture(raw, fallbackProject) {
  const parts = raw.trim().split(/\s+/).filter(Boolean);
  let project = fallbackProject;
  let thread;
  const title = [];
  for (let i = 0; i < parts.length; i += 1) {
    const part = parts[i];
    if (part === "!p") {
      const argument = parts[i + 1];
      if (!argument || argument.startsWith("!")) {
        project = null;
      } else {
        i += 1;
        project = argument;
      }
      continue;
    }
    if (part === "!t") {
      const argument = parts[i + 1];
      if (!argument || argument.startsWith("!")) {
        thread = null;
      } else {
        i += 1;
        thread = normalizeThread(argument);
      }
      continue;
    }
    title.push(part);
  }
  return { title: title.join(" "), project, thread };
}
