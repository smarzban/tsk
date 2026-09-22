/**
 * Turns inline `code` that names a key into <kbd>, so the docs can paint keys as
 * keycaps and flags, paths, and commands as plain code. Authors keep writing
 * backticks; only the rendered element changes.
 */
const NAMED = new Set([
  'Enter', 'Esc', 'Tab', 'Delete', 'Space', 'Backspace', 'wheel',
  '↑', '↓', '←', '→',
]);
const CHORD = /^(ctrl|shift|alt|cmd|meta)(\+(ctrl|shift|alt|cmd|meta))*\+([A-Za-z0-9]|Enter|Esc|Tab|Delete|↑|↓|←|→)$/i;
// Single characters the board, task page, capture, and recovery bind. Anything else
// in one-character code (`T`, `i`, `-`) is an identifier, not a key.
const SINGLE = new Set(['h', 'j', 'k', 'l', 'z', 'P', 'g', 'q', 'r', 'c', 's', 'p', 'e', 'u', '+', ':', '?', '1', '2', '3']);

export function isKeyName(text) {
  return NAMED.has(text) || CHORD.test(text) || SINGLE.has(text);
}

function textOf(node) {
  return (node.children || []).map((c) => (c.type === 'text' ? c.value : textOf(c))).join('');
}

function walk(node, inPre) {
  if (!node.children) return;
  for (const child of node.children) {
    if (child.type !== 'element') continue;
    if (child.tagName === 'code' && !inPre && isKeyName(textOf(child))) {
      child.tagName = 'kbd';
      continue;
    }
    walk(child, inPre || child.tagName === 'pre');
  }
}

export function rehypeKbd() {
  return (tree) => walk(tree, false);
}
