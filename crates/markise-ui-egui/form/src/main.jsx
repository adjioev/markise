// PROTOTYPE (proto/block-forms): the block edit dialog page.
//
// Runs in the docked edit panel of Markise (one in-process WKWebView, form_panel.rs). Markise generates
// the JSON Schema + UI Schema + data of one block; this page only renders them with JSON Forms
// (vanilla renderers) and talks to the helper through `window.webkit.messageHandlers.markise`:
//
//   page → helper: {event: "ready"} | {event: "shown", id, ms} | {event: "save", id, data}
//                  | {event: "cancel", id}
//   helper → page: window.markiseForm.open(req) | .result({ok, error}) | .close()
//
// Site extension API (version 1): a site's `<name>.form.js` calls
//   window.MarkiseForms.register(name, tester, renderer)
// with a JSON Forms tester and a React component (use `MarkiseForms.h` = React.createElement,
// `MarkiseForms.core` = @jsonforms/core, `MarkiseForms.react` = @jsonforms/react).

import React, { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import * as core from '@jsonforms/core';
import * as jfreact from '@jsonforms/react';
import { JsonForms, withJsonFormsControlProps } from '@jsonforms/react';
import './form.css';
import { vanillaCells, vanillaRenderers } from '@jsonforms/vanilla-renderers';

const h = React.createElement;

function post(m) {
  try {
    window.webkit.messageHandlers.markise.postMessage(JSON.stringify(m));
  } catch (e) {
    console.log('post', m, e);
  }
}

const T = {
  ru: {
    save: 'Сохранить',
    cancel: 'Отмена',
    add: 'Добавить',
    remove: 'Удалить',
    up: 'Выше',
    down: 'Ниже',
    saving: 'Сохраняю…',
    fix: 'Исправьте ошибки в форме',
    hint: 'Esc — отмена, ⌘↩ — сохранить',
    edit: 'правка блока',
    addAnswer: 'Добавить ответ',
    correct: 'верный',
    answerText: 'Текст ответа',
    picture: 'Картинка',
    alt: 'Описание (alt)',
    path: 'Путь к файлу',
    choose: 'Выбрать…',
    addPicture: 'Добавить картинку',
    noPreview: 'нет превью',
  },
  en: {
    save: 'Save',
    cancel: 'Cancel',
    add: 'Add',
    remove: 'Remove',
    up: 'Up',
    down: 'Down',
    saving: 'Saving…',
    fix: 'Fix the errors in the form',
    hint: 'Esc cancels, ⌘↩ saves',
    edit: 'edit block',
    addAnswer: 'Add answer',
    correct: 'correct',
    answerText: 'Answer text',
    picture: 'Picture',
    alt: 'Description (alt)',
    path: 'File path',
    choose: 'Choose…',
    addPicture: 'Add picture',
    noPreview: 'no preview',
  },
};

let currentLang = 'ru';
const tr = (k) => (T[currentLang] || T.en)[k] || k;

// ---- list of strings within minItems/maxItems (vanilla's table array needs window.confirm)
function ListControl({ data, handleChange, path, label, schema, enabled, visible, errors }) {
  if (!visible) return null;
  const items = Array.isArray(data) ? data : [];
  const min = schema.minItems ?? 0;
  const max = schema.maxItems ?? Infinity;
  const set = (next) => handleChange(path, next);
  return h(
    'div',
    { className: 'control mk-list' },
    h('label', null, label),
    items.map((v, i) =>
      h(
        'div',
        { className: 'mk-row', key: i },
        h('input', {
          className: 'input',
          value: v ?? '',
          disabled: !enabled,
          onChange: (e) => set(items.map((x, j) => (j === i ? e.target.value : x))),
        }),
        h(
          'button',
          {
            type: 'button',
            className: 'mk-icon',
            title: tr('up'),
            disabled: !enabled || i === 0,
            onClick: () => {
              const n = items.slice();
              [n[i - 1], n[i]] = [n[i], n[i - 1]];
              set(n);
            },
          },
          '↑',
        ),
        h(
          'button',
          {
            type: 'button',
            className: 'mk-icon',
            title: tr('down'),
            disabled: !enabled || i === items.length - 1,
            onClick: () => {
              const n = items.slice();
              [n[i + 1], n[i]] = [n[i], n[i + 1]];
              set(n);
            },
          },
          '↓',
        ),
        h(
          'button',
          {
            type: 'button',
            className: 'mk-icon',
            title: tr('remove'),
            disabled: !enabled || items.length <= min,
            onClick: () => set(items.filter((_, j) => j !== i)),
          },
          '✕',
        ),
      ),
    ),
    h(
      'button',
      {
        type: 'button',
        className: 'mk-add',
        disabled: !enabled || items.length >= max,
        onClick: () => set([...items, '']),
      },
      '+ ' + tr('add'),
    ),
    errors ? h('div', { className: 'validation' }, errors) : null,
  );
}

// ---- answers of a question: text, correct (radio for one answer, checkbox otherwise),
// order, add/remove; each answer keeps its {#id} label, new ones get the next free aN
function ChoicesControl({ data, handleChange, path, label, schema, uischema, enabled, visible, errors }) {
  if (!visible) return null;
  const items = Array.isArray(data) ? data : [];
  const o = uischema.options || {};
  const single = !!o.single;
  const min = schema.minItems ?? 1;
  const set = (next) => handleChange(path, next);
  const nextId = () => {
    const used = new Set([...(o.taken || []), ...items.map((x) => x.id)]);
    let n = 1;
    while (used.has('a' + n)) n++;
    return 'a' + n;
  };
  const move = (i, d) => {
    const n = items.slice();
    [n[i + d], n[i]] = [n[i], n[i + d]];
    set(n);
  };
  return h(
    'div',
    { className: 'control mk-choices' },
    h('label', null, label),
    items.map((v, i) =>
      h(
        'div',
        { className: 'mk-answer' + (v.correct ? ' is-correct' : ''), key: v.id },
        h(
          'label',
          { className: 'mk-correct', title: tr('correct') },
          h('input', {
            type: single ? 'radio' : 'checkbox',
            name: path + '-correct',
            checked: !!v.correct,
            disabled: !enabled,
            onChange: (e) =>
              set(items.map((x, j) => (single ? { ...x, correct: j === i } : j === i ? { ...x, correct: e.target.checked } : x))),
          }),
        ),
        h('input', {
          className: 'input',
          value: v.text ?? '',
          placeholder: tr('answerText'),
          disabled: !enabled,
          onChange: (e) => set(items.map((x, j) => (j === i ? { ...x, text: e.target.value } : x))),
        }),
        h('span', { className: 'mk-id' }, '{#' + v.id + '}'),
        h('button', { type: 'button', className: 'mk-icon', title: tr('up'), disabled: !enabled || i === 0, onClick: () => move(i, -1) }, '↑'),
        h('button', { type: 'button', className: 'mk-icon', title: tr('down'), disabled: !enabled || i === items.length - 1, onClick: () => move(i, 1) }, '↓'),
        h('button', { type: 'button', className: 'mk-icon', title: tr('remove'), disabled: !enabled || items.length <= min, onClick: () => set(items.filter((_, j) => j !== i)) }, '✕'),
      ),
    ),
    h(
      'button',
      { type: 'button', className: 'mk-add', disabled: !enabled, onClick: () => set([...items, { id: nextId(), text: '', correct: false }]) },
      '+ ' + tr('addAnswer'),
    ),
    errors ? h('div', { className: 'validation' }, errors) : null,
  );
}

// ---- pictures: one sub-card each (preview, alt, path + file picker), add/remove
let currentBase = '';
function Thumb({ src }) {
  const [bad, setBad] = useState(false);
  useEffect(() => setBad(false), [src]);
  let url = '';
  try {
    url = src ? new URL(src.replace(/^<|>$/g, ''), currentBase || undefined).href : '';
  } catch (e) {
    url = '';
  }
  if (!url || bad) return h('div', { className: 'mk-thumb mk-thumb-empty' }, tr('noPreview'));
  return h('img', { className: 'mk-thumb', src: url, alt: '', onError: () => setBad(true) });
}
function PicturesControl({ data, handleChange, path, label, schema, enabled, visible, errors }) {
  if (!visible) return null;
  const items = Array.isArray(data) ? data : [];
  const min = schema.minItems ?? 0;
  const max = schema.maxItems ?? Infinity;
  const set = (next) => handleChange(path, next);
  const upd = (i, k, v) => set(items.map((x, j) => (j === i ? { ...x, [k]: v } : x)));
  return h(
    'div',
    { className: 'control mk-pictures' },
    h('label', null, label),
    items.map((p, i) =>
      h(
        'div',
        { className: 'mk-pic', key: i },
        h(
          'div',
          { className: 'mk-pic-head' },
          h('span', null, tr('picture') + ' ' + (i + 1)),
          h('button', { type: 'button', className: 'mk-icon', title: tr('remove'), disabled: !enabled || items.length <= min, onClick: () => set(items.filter((_, j) => j !== i)) }, '✕'),
        ),
        h(Thumb, { src: p.src }),
        h('label', { className: 'mk-sub' }, tr('alt')),
        h('textarea', { className: 'input', rows: 2, value: p.alt ?? '', disabled: !enabled, onChange: (e) => upd(i, 'alt', e.target.value.replace(/\n/g, ' ')) }),
        h('label', { className: 'mk-sub' }, tr('path')),
        h(
          'div',
          { className: 'mk-row' },
          h('input', { className: 'input', value: p.src ?? '', disabled: !enabled, onChange: (e) => upd(i, 'src', e.target.value) }),
          h('button', { type: 'button', disabled: !enabled, onClick: () => post({ event: 'pick', id: window.__mkFormId, path, index: i }) }, tr('choose')),
        ),
      ),
    ),
    h('button', { type: 'button', className: 'mk-add', disabled: !enabled || items.length >= max, onClick: () => set([...items, { alt: '', src: '' }]) }, '+ ' + tr('addPicture')),
    errors ? h('div', { className: 'validation' }, errors) : null,
  );
}
const picturesTester = core.rankWith(30, core.and(core.isControl, core.optionIs('format', 'pictures')));

const choicesTester = core.rankWith(30, core.and(core.isControl, core.optionIs('format', 'choices')));

const listTester = core.rankWith(
  10,
  core.and(
    core.isControl,
    core.schemaMatches((s) => s.type === 'array' && s.items && s.items.type === 'string'),
  ),
);

const baseRenderers = [
  ...vanillaRenderers,
  { tester: listTester, renderer: withJsonFormsControlProps(ListControl) },
  { tester: choicesTester, renderer: withJsonFormsControlProps(ChoicesControl) },
  { tester: picturesTester, renderer: withJsonFormsControlProps(PicturesControl) },
];

// ---- site renderers
const site = new Map();
let bump = () => {};
window.MarkiseForms = {
  api: 1,
  React,
  h,
  core,
  react: jfreact,
  withJsonFormsControlProps,
  register(name, tester, renderer) {
    site.set(name, { tester, renderer: renderer.$$typeof ? renderer : withJsonFormsControlProps(renderer) });
    bump();
  },
};

function applyTheme(req) {
  const html = document.documentElement;
  html.dataset.theme = req.theme === 'dark' ? 'dark' : 'light';
  html.setAttribute('style', (req.vars || '') + ';color-scheme:' + html.dataset.theme);
  let css = document.getElementById('mk-site-css');
  if (!css) {
    css = document.createElement('style');
    css.id = 'mk-site-css';
    document.head.appendChild(css);
  }
  css.textContent = req.site_css || '';
}

function App() {
  const [req, setReq] = useState(null);
  const [data, setData] = useState(null);
  const [errors, setErrors] = useState([]);
  const [msg, setMsg] = useState(null);
  const [busy, setBusy] = useState(false);
  const [gen, setGen] = useState(0);

  useEffect(() => {
    bump = () => setGen((g) => g + 1);
    window.markiseForm = {
      open(r) {
        r._t = performance.now();
        currentLang = r.lang === 'en' ? 'en' : 'ru';
        currentBase = r.base || '';
        window.__mkFormId = r.id;
        applyTheme(r);
        setReq(r);
        setData(r.data);
        setErrors([]);
        setMsg(r.message || null);
        setBusy(false);
      },
      result(res) {
        setBusy(false);
        if (!res.ok) setMsg(res.error || 'error');
      },
      close() {
        setReq(null);
      },
      // measurement/test hook: current data
      data: () => dataRef.current,
      set(path, value) {
        setData((d) => {
          const n = JSON.parse(JSON.stringify(d));
          let o = n;
          const p = path.split('.');
          for (let i = 0; i < p.length - 1; i++) o = o[p[i]];
          o[p[p.length - 1]] = value;
          return n;
        });
      },
      save: () => saveRef.current(),
      // a file chosen in Markise's open panel for a picture
      picked(path, index, value) {
        setData((d) => {
          const n = JSON.parse(JSON.stringify(d));
          if (n[path] && n[path][index]) n[path][index].src = value;
          return n;
        });
      },
    };
    post({ event: 'ready' });
  }, []);

  useEffect(() => {
    if (!req) return;
    document.title = req.title || 'Markise';
    // a hidden WKWebView may not run animation frames: the timer is the fallback
    let done = false;
    const fire = () => {
      if (!done) {
        done = true;
        post({ event: 'shown', id: req.id, ms: performance.now() - req._t });
      }
    };
    requestAnimationFrame(() => requestAnimationFrame(fire));
    setTimeout(fire, 400);
    const first = document.querySelector('.mk-body input:not([disabled]), .mk-body textarea:not([disabled])');
    if (first) first.focus();
  }, [req]);

  const save = () => {
    if (!req || busy) return;
    if (errors.length) {
      setMsg(tr('fix'));
      return;
    }
    setBusy(true);
    setMsg(null);
    post({ event: 'save', id: req.id, data });
  };
  const cancel = () => req && post({ event: 'cancel', id: req.id });
  saveRef.current = save;
  dataRef.current = data;

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        cancel();
      } else if ((e.metaKey || e.ctrlKey) && (e.key === 'Enter' || e.key === 's')) {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const renderers = useMemo(() => [...baseRenderers, ...site.values()], [gen, req]);

  if (!req) return h('div', { className: 'mk-empty' });
  return h(
    'div',
    { className: 'mk-form', 'data-block': req.block },
    h(
      'header',
      null,
      h('div', { className: 'mk-title' }, req.title, h('span', null, ' — ' + tr('edit'))),
      req.note ? h('div', { className: 'mk-note' }, req.note) : null,
    ),
    h(
      'main',
      { className: 'mk-body' },
      h(JsonForms, {
        schema: req.schema,
        uischema: req.uischema,
        data,
        renderers,
        cells: vanillaCells,
        validationMode: 'ValidateAndShow',
        onChange: ({ data: d, errors: e }) => {
          setData(d);
          setErrors(e || []);
        },
      }),
    ),
    h(
      'footer',
      null,
      msg ? h('div', { className: 'mk-msg' }, msg) : h('div', { className: 'mk-hint' }, tr('hint')),
      h('button', { type: 'button', onClick: cancel }, tr('cancel')),
      h('button', { type: 'button', className: 'mk-primary', onClick: save, disabled: busy }, busy ? tr('saving') : tr('save')),
    ),
  );
}

const saveRef = { current: () => {} };
const dataRef = { current: null };

createRoot(document.getElementById('root')).render(h(App));
