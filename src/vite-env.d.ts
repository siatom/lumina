/// <reference types="vite/client" />

/**
 * Vite turns `import icon from "./x.svg"` into a URL string (a hashed asset URL
 * in a build, a dev-server URL in `pnpm dev`). TypeScript needs to be told that,
 * which is what the reference above does.
 *
 * The reference only works when `vite/client` is resolvable, so the SVG case is
 * also declared explicitly — it is the only asset kind this project imports.
 */
declare module "*.svg" {
  const src: string;
  export default src;
}
