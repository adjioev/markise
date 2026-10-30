// PROTOTYPE stub "site code" for :::question (original work, MIT OR Apache-2.0).
// Contract with markise-web-render: register `markiseBlocks[<component>] = decorate(block)`;
// the snapshot is taken after the returned promise resolves ("decorated").
(function () {
  window.markiseBlocks = window.markiseBlocks || {};
  window.markiseBlocks.question = async function decorate(block) {
    const label = /\s*\{#([A-Za-z0-9_-]+)\}\s*$/;
    for (const li of block.querySelectorAll('li.task-list-item')) {
      const input = li.querySelector('input[type="checkbox"]');
      const correct = !!(input && input.checked);
      if (input) input.remove();
      // hide the {#id} label of the answer, keep it as data
      const walker = document.createTreeWalker(li, NodeFilter.SHOW_TEXT);
      let node, last = null;
      while ((node = walker.nextNode())) last = node;
      if (last) {
        const m = last.nodeValue.match(label);
        if (m) {
          li.dataset.answer = m[1];
          last.nodeValue = last.nodeValue.replace(label, '');
        }
      }
      li.classList.add('mk-answer');
      const box = document.createElement('span');
      box.className = 'mk-box';
      li.prepend(box);
      if (correct) {
        li.classList.add('mk-correct');
        const badge = document.createElement('span');
        badge.className = 'mk-badge';
        badge.textContent = '✓ correct';
        li.append(badge);
      }
    }
    // wrap the answer text so the flex layout keeps it in one column
    for (const li of block.querySelectorAll('li.mk-answer')) {
      const text = document.createElement('span');
      for (const n of [...li.childNodes]) {
        if (n.nodeType === 1 && (n.classList.contains('mk-box') || n.classList.contains('mk-badge'))) continue;
        text.append(n);
      }
      li.insertBefore(text, li.querySelector('.mk-badge'));
    }
  };
})();
