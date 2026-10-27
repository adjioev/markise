//! A small synthetic element tree that mirrors the live-editor DOM of spec 03 §3–§4.
//!
//! Theme selectors are matched against these nodes with real selector semantics (combinators,
//! classes, structural pseudo-classes), so `#write h1`, `h1`, `.md-heading` or
//! `content #write > h1` all reach the same heading node. Each native theme slot reads the
//! computed style of one well-known node.

pub(crate) struct VNode {
    pub tag: &'static str,
    pub id: Option<&'static str>,
    pub classes: Vec<&'static str>,
    pub attrs: Vec<(&'static str, &'static str)>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Holds text content (for `:empty`).
    pub has_text: bool,
}

impl VNode {
    pub fn attr(&self, name: &str) -> Option<&str> {
        if name == "class" {
            return None; // classes are matched through `.class`; `[class…]` is rare
        }
        if name == "id" {
            return self.id;
        }
        self.attrs.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
    }
}

/// cm token classes we model (spec 03 §4.5).
pub(crate) const CM_TOKENS: &[&str] = &[
    "cm-keyword",
    "cm-atom",
    "cm-number",
    "cm-def",
    "cm-variable",
    "cm-variable-2",
    "cm-variable-3",
    "cm-property",
    "cm-operator",
    "cm-comment",
    "cm-string",
    "cm-string-2",
    "cm-meta",
    "cm-qualifier",
    "cm-builtin",
    "cm-bracket",
    "cm-tag",
    "cm-attribute",
    "cm-header",
    "cm-quote",
    "cm-link",
    "cm-error",
];

pub(crate) const ALERTS: [&str; 5] = ["note", "tip", "important", "warning", "caution"];

pub(crate) struct Dom {
    pub nodes: Vec<VNode>,
    pub html: usize,
    pub body: usize,
    pub content: usize,
    pub write: usize,
    pub h: [usize; 6],
    pub p: usize,
    pub a: usize,
    pub code: usize,
    pub strong: usize,
    pub em: usize,
    pub del: usize,
    pub mark: usize,
    pub md_meta: usize,
    pub blockquote: usize,
    pub quote_p: usize,
    /// `ul` at nesting levels 1–3 and their first `li`.
    pub ul: [usize; 3],
    pub ul_li: [usize; 3],
    pub ol: usize,
    pub ol_li: usize,
    pub task_input_done: usize,
    pub task_input_open: usize,
    pub fences: usize,
    pub codemirror: usize,
    pub cm_line: usize,
    pub cm_tokens: Vec<(&'static str, usize)>,
    pub th: usize,
    pub thead_tr: usize,
    pub td: usize,
    pub tr_even: usize,
    pub td_even: usize,
    pub hr: usize,
    pub meta_block: usize,
    pub math: usize,
    pub alerts: [usize; 5],
    pub alert_titles: [usize; 5],
    pub sidebar: usize,
    /// Sidebar chrome (spec 03 §3.2): tab, active tab, file tree folder/file/active file
    /// rows and titles, outline items (h1, active h2, nested h3).
    pub side_tab: usize,
    pub side_tab_active: usize,
    pub tree_row: usize,
    pub tree_title: usize,
    pub tree_row_active: usize,
    pub tree_title_active: usize,
    pub tree_child_row: usize,
    pub outline_wrap_h1: usize,
    pub outline_item: usize,
    pub outline_label: usize,
    pub outline_item_active: usize,
    pub outline_label_active: usize,
    pub outline_wrap_h3: usize,
}

struct Builder {
    nodes: Vec<VNode>,
}

impl Builder {
    /// `spec` is `tag#id.class1.class2[attr=value][flag]`.
    fn add(&mut self, parent: Option<usize>, spec: &'static str) -> usize {
        let mut rest = spec;
        let end = rest.find(['#', '.', '[']).unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let mut id = None;
        let mut classes = Vec::new();
        let mut attrs = Vec::new();
        while !rest.is_empty() {
            let kind = rest.as_bytes()[0];
            rest = &rest[1..];
            if kind == b'[' {
                let close = rest.find(']').expect("attr");
                let body = &rest[..close];
                rest = &rest[close + 1..];
                match body.split_once('=') {
                    Some((k, v)) => attrs.push((k, v)),
                    None => attrs.push((body, "")),
                }
                continue;
            }
            let end = rest.find(['#', '.', '[']).unwrap_or(rest.len());
            let name = &rest[..end];
            rest = &rest[end..];
            if kind == b'#' {
                id = Some(name);
            } else {
                classes.push(name);
            }
        }
        let idx = self.nodes.len();
        self.nodes.push(VNode {
            tag,
            id,
            classes,
            attrs,
            parent,
            children: Vec::new(),
            has_text: false,
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(idx);
        }
        idx
    }

    fn text(&mut self, parent: usize, spec: &'static str) -> usize {
        let i = self.add(Some(parent), spec);
        self.nodes[i].has_text = true;
        i
    }
}

impl Dom {
    pub(crate) fn build() -> Dom {
        let mut b = Builder { nodes: Vec::new() };
        let html = b.add(None, "html");
        let body = b.add(Some(html), "body");
        let content = b.add(Some(body), "content");
        let write = b.add(Some(content), "div#write");
        // absorbs `#write > *:first-child` so generic blocks are not first children
        b.add(Some(write), "div.md-first-block-placeholder");

        // `.h1`…`.h6` alias classes (C5): rules written for `.hN` style the headings too
        let h_specs = [
            "h1.md-end-block.md-heading.h1[mdtype=heading]",
            "h2.md-end-block.md-heading.h2[mdtype=heading]",
            "h3.md-end-block.md-heading.h3[mdtype=heading]",
            "h4.md-end-block.md-heading.h4[mdtype=heading]",
            "h5.md-end-block.md-heading.h5[mdtype=heading]",
            "h6.md-end-block.md-heading.h6[mdtype=heading]",
        ];
        let mut h = [0; 6];
        for (i, s) in h_specs.iter().enumerate() {
            h[i] = b.text(write, s);
        }

        let p = b.text(write, "p.md-end-block.md-p[mdtype=paragraph]");
        let a = b.text(p, "a[href=#]");
        let code = b.text(p, "code");
        let strong = b.text(p, "strong");
        let em = b.text(p, "em");
        let del = b.text(p, "del");
        let mark = b.text(p, "mark");
        b.text(p, "u");
        b.text(p, "kbd");
        b.text(p, "sub");
        b.text(p, "sup");
        let md_meta = b.text(p, "span.md-meta.md-before");
        let img_span = b.add(Some(p), "span.md-image.md-img-loaded[md-inline=image]");
        b.add(Some(img_span), "img[src=x.png]");

        let blockquote = b.add(Some(write), "blockquote[mdtype=blockquote]");
        let quote_p = b.text(blockquote, "p.md-end-block.md-p[mdtype=paragraph]");

        let mut ul = [0; 3];
        let mut ul_li = [0; 3];
        let mut parent = write;
        for lvl in 0..3 {
            ul[lvl] = b.add(Some(parent), "ul.ul-list[data-mark=-][mdtype=list]");
            ul_li[lvl] = b.add(Some(ul[lvl]), "li.md-list-item[mdtype=list_item]");
            b.text(ul_li[lvl], "p.md-end-block.md-p[mdtype=paragraph]");
            parent = ul_li[lvl];
        }
        let ol = b.add(Some(write), "ol.ol-list[mdtype=list]");
        let ol_li = b.add(Some(ol), "li.md-list-item[mdtype=list_item]");
        b.text(ol_li, "p.md-end-block.md-p[mdtype=paragraph]");

        let task = b.add(Some(write), "ul.ul-list.task-list[data-mark=-][mdtype=list]");
        let li_done = b.add(
            Some(task),
            "li.md-list-item.task-list-item.md-task-list-item.task-list-done[mdtype=list_item]",
        );
        let task_input_done = b.add(Some(li_done), "input[type=checkbox][checked]");
        b.text(li_done, "p.md-end-block.md-p[mdtype=paragraph]");
        let li_open = b.add(
            Some(task),
            "li.md-list-item.task-list-item.md-task-list-item.task-list-not-done[mdtype=list_item]",
        );
        let task_input_open = b.add(Some(li_open), "input[type=checkbox]");
        b.text(li_open, "p.md-end-block.md-p[mdtype=paragraph]");

        let fences = b.add(
            Some(write),
            "pre.md-fences.mock-cm.md-end-block[lang=js][mdtype=fences]",
        );
        let codemirror = b.add(Some(fences), "div.CodeMirror.cm-s-inner.cm-s-typora-default");
        let lines = b.add(Some(codemirror), "div.CodeMirror-lines");
        let cm_line = b.add(Some(lines), "pre.CodeMirror-line");
        let span = b.add(Some(cm_line), "span[role=presentation]");
        let mut cm_tokens = Vec::new();
        for name in CM_TOKENS {
            let idx = b.text(span, "span");
            b.nodes[idx].classes.push(name);
            cm_tokens.push((*name, idx));
        }

        let fig = b.add(Some(write), "figure.md-table-fig.table-figure");
        let table = b.add(Some(fig), "table.md-table[mdtype=table]");
        let thead = b.add(Some(table), "thead");
        let thead_tr = b.add(Some(thead), "tr.md-end-block[mdtype=table_row]");
        let th = b.text(thead_tr, "th");
        b.text(thead_tr, "th");
        let tbody = b.add(Some(table), "tbody");
        let tr_odd = b.add(Some(tbody), "tr.md-end-block[mdtype=table_row]");
        let td = b.text(tr_odd, "td");
        b.text(tr_odd, "td");
        let tr_even = b.add(Some(tbody), "tr.md-end-block[mdtype=table_row]");
        let td_even = b.text(tr_even, "td");
        b.text(tr_even, "td");
        let tr3 = b.add(Some(tbody), "tr.md-end-block[mdtype=table_row]");
        b.text(tr3, "td");

        let hr_wrap = b.add(Some(write), "div.md-hr.md-end-block[mdtype=hr]");
        let hr = b.add(Some(hr_wrap), "hr");
        let meta_block = b.text(write, "pre.md-meta-block.md-end-block[mdtype=meta_block]");
        let math = b.add(
            Some(write),
            "div.mathjax-block.md-end-block.md-math-block.md-rawblock[mdtype=math_block]",
        );

        let alert_specs = [
            "div.md-alert.md-alert-note",
            "div.md-alert.md-alert-tip",
            "div.md-alert.md-alert-important",
            "div.md-alert.md-alert-warning",
            "div.md-alert.md-alert-caution",
        ];
        let title_specs = [
            "span.md-alert-text.md-alert-text-note",
            "span.md-alert-text.md-alert-text-tip",
            "span.md-alert-text.md-alert-text-important",
            "span.md-alert-text.md-alert-text-warning",
            "span.md-alert-text.md-alert-text-caution",
        ];
        let mut alerts = [0; 5];
        let mut alert_titles = [0; 5];
        for i in 0..5 {
            alerts[i] = b.add(Some(write), alert_specs[i]);
            let ap = b.add(Some(alerts[i]), "p.md-end-block.md-p");
            let cont = b.add(Some(ap), "span.md-alert-text-container");
            alert_titles[i] = b.text(cont, title_specs[i]);
        }

        let toc = b.add(Some(write), "div.md-toc.md-end-block[mdtype=toc]");
        let toc_p = b.add(Some(toc), "p.md-toc-content[role=list]");
        let toc_item = b.add(Some(toc_p), "span.md-toc-item.md-toc-h1");
        b.text(toc_item, "a.md-toc-inner[href=#]");
        // absorbs `:last-child`
        b.text(write, "p.md-end-block.md-p[mdtype=paragraph]");

        let sidebar = b.add(Some(body), "div#typora-sidebar");
        let tabs = b.add(Some(sidebar), "div.sidebar-tabs");
        let side_tab = b.text(tabs, "div.sidebar-tab.info-panel-tab#info-panel-tab-file");
        let side_tab_active = b.text(tabs, "div.sidebar-tab.info-panel-tab.active#info-panel-tab-outline");
        let lib = b.add(Some(sidebar), "div#file-library.file-library");
        let tree = b.add(Some(lib), "div#file-library-tree");
        let folder = b.add(
            Some(tree),
            "div.file-library-node.file-tree-node.file-node-expanded[data-is-directory=true]",
        );
        let tree_row = b.add(Some(folder), "div.file-node-content");
        b.add(Some(tree_row), "span.file-node-icon");
        let tree_title = b.text(tree_row, "span.file-node-title");
        let kids = b.add(Some(folder), "div.file-node-children");
        let file = b.add(Some(kids), "div.file-library-node.file-tree-node");
        let tree_child_row = b.add(Some(file), "div.file-node-content");
        b.text(tree_child_row, "span.file-node-title");
        let active = b.add(Some(kids), "div.file-library-node.file-tree-node.active");
        let tree_row_active = b.add(Some(active), "div.file-node-content");
        let tree_title_active = b.text(tree_row_active, "span.file-node-title");
        let outline = b.add(Some(sidebar), "div#outline-content.outline-content");
        let oul = b.add(Some(outline), "ul.outline-children");
        let li1 = b.add(Some(oul), "li");
        let outline_wrap_h1 = b.add(Some(li1), "div.outline-item-wrapper.outline-h1");
        let outline_item = b.add(Some(outline_wrap_h1), "div.outline-item");
        let outline_label = b.text(outline_item, "span.outline-label");
        let li2 = b.add(Some(oul), "li");
        let wrap2 = b.add(Some(li2), "div.outline-item-wrapper.outline-h2");
        let outline_item_active = b.add(Some(wrap2), "div.outline-item.outline-item-active");
        let outline_label_active = b.text(outline_item_active, "span.outline-label");
        let oul2 = b.add(Some(li2), "ul.outline-children");
        let li3 = b.add(Some(oul2), "li");
        let outline_wrap_h3 = b.add(Some(li3), "div.outline-item-wrapper.outline-h3");
        let item3 = b.add(Some(outline_wrap_h3), "div.outline-item");
        b.text(item3, "span.outline-label");

        Dom {
            nodes: b.nodes,
            html,
            body,
            content,
            write,
            h,
            p,
            a,
            code,
            strong,
            em,
            del,
            mark,
            md_meta,
            blockquote,
            quote_p,
            ul,
            ul_li,
            ol,
            ol_li,
            task_input_done,
            task_input_open,
            fences,
            codemirror,
            cm_line,
            cm_tokens,
            th,
            thead_tr,
            td,
            tr_even,
            td_even,
            hr,
            meta_block,
            math,
            alerts,
            alert_titles,
            sidebar,
            side_tab,
            side_tab_active,
            tree_row,
            tree_title,
            tree_row_active,
            tree_title_active,
            tree_child_row,
            outline_wrap_h1,
            outline_item,
            outline_label,
            outline_item_active,
            outline_label_active,
            outline_wrap_h3,
        }
    }

    pub(crate) fn prev_sibling(&self, node: usize) -> Option<usize> {
        let p = self.nodes[node].parent?;
        let sibs = &self.nodes[p].children;
        let i = sibs.iter().position(|&c| c == node)?;
        if i == 0 { None } else { Some(sibs[i - 1]) }
    }

    /// 1-based index of `node` among its siblings (of the same type if `of_type`) and the count.
    pub(crate) fn sibling_position(&self, node: usize, of_type: bool) -> (usize, usize) {
        let Some(p) = self.nodes[node].parent else {
            return (1, 1);
        };
        let tag = self.nodes[node].tag;
        let mut idx = 0;
        let mut count = 0;
        for &c in &self.nodes[p].children {
            if of_type && self.nodes[c].tag != tag {
                continue;
            }
            count += 1;
            if c == node {
                idx = count;
            }
        }
        (idx, count)
    }
}
