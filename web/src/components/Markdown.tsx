import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="prose-hangar">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ node: _, ...props }) => <a {...props} target="_blank" rel="noreferrer noopener" />,
          // Never load images from model output: a remote image is a request
          // to any server the text names. Show a link instead.
          img: ({ src, alt }) =>
            typeof src === "string" && src ? (
              <a href={src} target="_blank" rel="noreferrer noopener">
                [image: {alt || src}]
              </a>
            ) : null,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
