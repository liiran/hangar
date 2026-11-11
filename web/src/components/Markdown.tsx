import { memo, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { linkHost, webUrl } from "../safe";

/** The plain text inside a rendered node. */
function textOf(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (node && typeof node === "object" && "props" in node) return textOf((node.props as { children?: ReactNode }).children);
  return "";
}

/**
 * A link out of the app. Only web addresses become links (anything else
 * stays text), they open in a new tab that cannot reach this window, and
 * when the text does not name the site it really goes to, the site is shown.
 */
function ExternalLink({ href, children }: { href: unknown; children: ReactNode }) {
  const url = webUrl(href);
  if (!url) return <span>{children}</span>;
  const host = linkHost(url, textOf(children));
  return (
    <a href={url} target="_blank" rel="noreferrer noopener" title={url}>
      {children}
      {host && <span className="link-host"> ({host})</span>}
    </a>
  );
}

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="prose-hangar">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        urlTransform={(url) => webUrl(url) ?? ""}
        components={{
          a: ({ href, children }) => <ExternalLink href={href}>{children}</ExternalLink>,
          // Never load images from model output: a remote image is a request
          // to any server the text names. Show a link instead.
          img: ({ src, alt }) => <ExternalLink href={src}>[image: {alt || (typeof src === "string" && src) || "no address"}]</ExternalLink>,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
