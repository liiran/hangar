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
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
