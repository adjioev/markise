// PROTOTYPE stub "site code" (original work, MIT OR Apache-2.0): a site-provided JSON Forms
// renderer for the Markise block edit dialog — a single-line field with a character counter,
// used by controls whose options say {"format": "counted", "maxLength": N}.
(function () {
  const M = window.MarkiseForms;
  if (!M || M.api !== 1) return;
  const { h, core } = M;
  const tester = core.rankWith(20, core.and(core.isControl, core.optionIs('format', 'counted')));
  function Counted({ data, handleChange, path, label, uischema, enabled, visible }) {
    if (!visible) return null;
    const max = (uischema.options && uischema.options.maxLength) || 100;
    const n = (data || '').length;
    return h('div', { className: 'control site-counted' },
      h('label', null, label),
      h('textarea', {
        className: 'input', rows: 2, value: data || '', disabled: !enabled,
        onChange: (e) => handleChange(path, e.target.value.replace(/\n/g, ' ')),
      }),
      h('div', { style: { fontSize: '11px', textAlign: 'right', color: n > max ? '#d33' : 'var(--muted)' } },
        n + ' / ' + max + (n > max ? ' — длинновато для экрана телефона' : '')));
  }
  M.register('site-counted', tester, Counted);
})();
