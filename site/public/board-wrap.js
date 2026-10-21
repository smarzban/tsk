// Same word-boundary rule as ui/edit.rs: retain whitespace at a soft break;
// hard-break only a token that cannot fit. Width is measured in painted cells.
function displayCells(character) {
  return /\p{Mark}/u.test(character)
    ? 0
    : /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\p{Extended_Pictographic}]/u.test(
          character,
        )
      ? 2
      : 1;
}

export function wrapText(value, width) {
  width = Math.max(1, width);
  return String(value)
    .split(/\r\n|\r|\n/)
    .flatMap((line) => {
      const chars = [...line];
      const rows = [];
      let start = 0;
      while (start < chars.length) {
        let end = start,
          used = 0,
          soft;
        while (end < chars.length) {
          const size = displayCells(chars[end]);
          if (used + size > width && end > start) {
            end = soft ?? end;
            break;
          }
          used += size;
          if (/\s/u.test(chars[end])) soft = end + 1;
          end++;
          if (used > width) break;
        }
        rows.push(chars.slice(start, end).join(""));
        start = end;
      }
      return rows.length ? rows : [""];
    });
}
