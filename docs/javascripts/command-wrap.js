// Long commands wrap in code blocks instead of scrolling out of view. Browsers may break a
// line after any hyphen, which splits `--delivery` into `-` and `-delivery`; keep each word whole so
// lines break only at spaces. The text, and what the copy button copies, is unchanged.
(() => {
  const BLOCKS = '.md-typeset pre > code';

  function keepWordsWhole(code) {
    if (code.dataset.wordsWhole) return;
    code.dataset.wordsWhole = 'true';
    const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    for (const node of nodes) {
      if (!/\S-|-\S/.test(node.data)) continue;
      const fragment = document.createDocumentFragment();
      for (const part of node.data.split(/(\s+)/)) {
        if (!part) continue;
        if (/\s/.test(part) || !part.includes('-')) {
          fragment.append(part);
        } else {
          const word = document.createElement('span');
          word.className = 'docs-word';
          word.textContent = part;
          fragment.append(word);
        }
      }
      node.replaceWith(fragment);
    }
  }

  const run = () => document.querySelectorAll(BLOCKS).forEach(keepWordsWhole);
  if (window.document$) window.document$.subscribe(run);
  else document.addEventListener('DOMContentLoaded', run);
})();
