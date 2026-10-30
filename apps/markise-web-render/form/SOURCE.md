# Block edit form page (PROTOTYPE, branch `proto/block-forms`)

- `src/main.jsx`, `src/form.css`, `build.sh`, `package.json`: original work (MIT OR Apache-2.0).
- `dist/form.js`, `dist/form.css`: built by `sh build.sh` (esbuild) from `src/` and these npm
  packages, bundled for offline use (no CDN); their licence comments are kept at the end of
  `dist/form.js`:

| Package | Version | Licence |
|---|---|---|
| @jsonforms/core, @jsonforms/react, @jsonforms/vanilla-renderers | 3.8.0 | MIT |
| react, react-dom | 18.3.1 | MIT |
| scheduler | 0.23.2 | MIT |
| ajv | 8.20.0 | MIT |
| ajv-formats | 2.1.1 | MIT |
| fast-deep-equal | 3.1.3 | MIT |
| fast-uri | 3.1.9 | BSD-3-Clause |
| json-schema-traverse | 1.0.0 | MIT |
| require-from-string | 2.0.2 | MIT |
| lodash | 4.18.1 | MIT |
| loose-envify, js-tokens | 1.4.0, 4.0.0 | MIT |
| esbuild (build only) | 0.25.12 | MIT |
