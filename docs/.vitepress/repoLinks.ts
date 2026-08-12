import fs from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";

/**
 * The docs folder doubles as the source for rozi.tui-lipan.dev, so its pages link
 * to repository files that are not part of the site - `../AGENTS.md`,
 * `../examples/config.toml`, `../integrations/…`. Those resolve on GitHub and
 * inside an editor, but they are dead links once only `docs/` is published.
 *
 * This plugin rewrites exactly those targets to GitHub URLs before VitePress
 * parses the markdown, which keeps the links working in both places and keeps
 * the site's dead-link check meaningful for genuinely broken in-docs links.
 *
 * It does the same for links to a directory that has no index page, such as
 * `audits/`. Those are requests for a file listing, which only GitHub can give.
 */

const REPO = "https://github.com/tui-lipan/rozi";
const BRANCH = "master";
const FRAMEWORK_REPO = "https://github.com/tui-lipan/tui-lipan";

/** Sibling checkouts a doc may point at with `../../<name>`. */
const SIBLING_REPOS: Record<string, string> = {
  "tui-lipan": FRAMEWORK_REPO,
};

/** `](target)` inline links and `[ref]: target` reference definitions. */
const INLINE_LINK = /(\]\()([^()\s]+?)(\s+"[^"]*")?(\))/g;
const REFERENCE_LINK = /^(\[[^\]]+\]:\s*)(\S+)$/gm;

function resolveFromRepoRoot(target: string, pageRelPath: string): string {
  const pageDir = path.posix.dirname(path.posix.join("docs", pageRelPath));
  return path.posix.normalize(path.posix.join(pageDir, target));
}

/**
 * Returns the GitHub URL for a link that escapes `docs/`, or `null` when the
 * link is internal, absolute, or otherwise none of our business.
 */
export function githubUrlFor(
  target: string,
  pageRelPath: string,
  srcDir?: string,
): string | null {
  if (/^[a-z]+:|^[/#]/i.test(target)) return null;

  const [pathPart, hash = ""] = target.split("#", 2);
  if (!pathPart) return null;
  const suffix = hash ? `#${hash}` : "";
  const resolved = resolveFromRepoRoot(pathPart, pageRelPath);

  if (resolved.startsWith("docs/")) {
    // Inside the site. Only a directory without an index page needs GitHub -
    // that link is asking for a file listing, which the site cannot render.
    if (!pathPart.endsWith("/") || !srcDir) return null;
    const dir = path.join(srcDir, resolved.slice("docs/".length));
    const hasIndex = ["index.md", "README.md"].some((name) =>
      fs.existsSync(path.join(dir, name)),
    );
    if (hasIndex) return null;
    return `${REPO}/tree/${BRANCH}/${resolved.replace(/\/$/, "")}${suffix}`;
  }

  // Escaped the repository entirely - a sibling checkout such as ../../tui-lipan.
  if (resolved.startsWith("../")) {
    const sibling = resolved.replace(/^(\.\.\/)+/, "").split("/")[0];
    const repo = SIBLING_REPOS[sibling];
    return repo ? repo + suffix : null;
  }

  // A trailing slash or an extensionless segment means a directory listing.
  const isDirectory =
    pathPart.endsWith("/") || !path.posix.basename(resolved).includes(".");
  const kind = isDirectory ? "tree" : "blob";
  return `${REPO}/${kind}/${BRANCH}/${resolved.replace(/\/$/, "")}${suffix}`;
}

export function rewriteRepoLinks(
  source: string,
  pageRelPath: string,
  srcDir?: string,
): string {
  return source
    .replace(INLINE_LINK, (match, open, target, title = "", close) => {
      const url = githubUrlFor(target, pageRelPath, srcDir);
      return url ? `${open}${url}${title}${close}` : match;
    })
    .replace(REFERENCE_LINK, (match, label, target) => {
      const url = githubUrlFor(target, pageRelPath, srcDir);
      return url ? `${label}${url}` : match;
    });
}

export function repoLinksPlugin(srcDir: string): Plugin {
  return {
    name: "rozi:repo-links",
    enforce: "pre",
    transform(code, id) {
      const [file] = id.split("?", 1);
      if (!file.endsWith(".md")) return null;
      const relative = path.posix.relative(srcDir, file.split(path.sep).join("/"));
      if (relative.startsWith("..")) return null;
      return rewriteRepoLinks(code, relative, srcDir);
    },
  };
}
