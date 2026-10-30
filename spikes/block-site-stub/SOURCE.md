# block-site-stub

Throwaway stub "site code" for the block-snapshot prototype (branch `proto/block-snapshots`):
`blocks/goals/goals.css`, `blocks/question/question.css`, `blocks/question/question.js` are
original work written for this prototype (MIT OR Apache-2.0).

Markise looks for block code in the site repository the document lives in (nearest ancestor
folder with `components/blocks.json`, then `blocks/<name>/<name>.{css,js}`); this folder is the
fallback, selected with `MARKISE_BLOCK_SITE=<this folder>`.
