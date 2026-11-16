const TOKEN = new Set(["!p", "!t", "!a", "!b", "!d", "!w"]);

function normalizeName(raw, kind) {
  const input = String(raw);
  if (!input) return { refuse: `${kind} name is empty` };
  if (!/^[A-Za-z0-9]/.test(input)) {
    return { refuse: `${kind} must start with a letter or number` };
  }
  if (!/^[A-Za-z0-9][A-Za-z0-9.-]*$/.test(input)) {
    return { refuse: `${kind}: use letters, numbers, hyphens, and dots` };
  }
  if ([...input].length > 32) {
    return { refuse: `${kind} is at most 32 characters` };
  }
  return { value: input.toLowerCase() };
}

function normalizeThread(raw) {
  return normalizeName(raw, "thread");
}

function normalizeAssignee(raw) {
  let name = String(raw).trim();
  if (name.startsWith("@")) name = name.slice(1);
  if (/^(me|you)$/i.test(name)) return { value: "you" };
  return normalizeName(name, "assignee");
}

function tokenArgument(parts, index) {
  const next = parts[index + 1];
  if (!next || TOKEN.has(next) || next.startsWith("#")) return undefined;
  return next;
}

// A real day, `YYYY-MM-DD` or with a one-digit month or day, year 0001 to 9999, worded
// like the board's refusals.
function dueRefusal(value) {
  const shaped = /^\d{4}-\d{1,2}-\d{1,2}$/.test(value);
  if (!shaped) return `due ${value}: a due date is a day written YYYY-MM-DD`;
  const [year, month, day] = value.split("-").map(Number);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  const real = year > 0 && month >= 1 && month <= 12 && day >= 1;
  if (!real || day > days[month - 1]) {
    return `due ${value}: that day does not exist in the calendar`;
  }
  return "";
}

function parseAfterNumber(value) {
  const digits = value.replace(/^[Tt]/, "");
  if (!digits || !/^\d+$/.test(digits)) return null;
  const number = Number(digits);
  return number > 0 ? number : null;
}

export function parseCapture(raw, fallbackProject) {
  const parts = String(raw).trim().split(/\s+/).filter(Boolean);
  let project = fallbackProject;
  let thread;
  let assignee;
  let base;
  let due;
  const after = [];
  const title = [];
  let refuse = "";

  for (let i = 0; i < parts.length; i += 1) {
    const part = parts[i];
    if (part === "!p") {
      const argument = tokenArgument(parts, i);
      if (argument === undefined) {
        project = null;
      } else {
        i += 1;
        project = argument;
      }
      continue;
    }
    if (part === "!t") {
      const argument = tokenArgument(parts, i);
      if (argument === undefined) {
        thread = null;
      } else {
        i += 1;
        const normalized = normalizeThread(argument);
        if (normalized.refuse) {
          refuse = normalized.refuse;
          thread = null;
          break;
        }
        thread = normalized.value;
      }
      continue;
    }
    if (part === "!a") {
      const argument = tokenArgument(parts, i);
      if (argument === undefined) {
        assignee = null;
      } else {
        i += 1;
        const normalized = normalizeAssignee(argument);
        if (normalized.refuse) {
          refuse = normalized.refuse;
          assignee = null;
          break;
        }
        assignee = normalized.value;
      }
      continue;
    }
    if (part === "!b") {
      const argument = tokenArgument(parts, i);
      if (argument === undefined) {
        base = null;
      } else {
        i += 1;
        base = argument;
      }
      continue;
    }
    if (part === "!d") {
      const argument = tokenArgument(parts, i);
      if (argument === undefined) {
        refuse = "!d needs a day like 2026-11-30";
        break;
      }
      i += 1;
      refuse = dueRefusal(argument);
      if (refuse) break;
      due = argument
        .split("-")
        .map((part, index) => part.padStart(index === 0 ? 4 : 2, "0"))
        .join("-");
      continue;
    }
    if (part === "!w") {
      const argument = tokenArgument(parts, i);
      const number = argument ? parseAfterNumber(argument) : null;
      if (number === null) {
        refuse = "!w needs a task number like T12";
        break;
      }
      i += 1;
      if (!after.includes(number)) after.push(number);
      continue;
    }
    title.push(part);
  }

  if (!refuse && base && !project) {
    refuse = "base requires a project task";
  }

  return {
    title: title.join(" "),
    project,
    thread,
    assignee,
    base,
    due,
    after,
    refuse,
  };
}
