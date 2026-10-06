// A text field over a parsed value (a number, a list of tags): the value is
// committed as the user types, but the text shown is only normalized on
// blur, so "0," or "código," can be typed.

import { type InputHTMLAttributes, useEffect, useState } from "react";

export function DraftInput({
  value,
  onCommit,
  ...props
}: { value: string; onCommit: (text: string) => void } & Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange"
>) {
  const [text, setText] = useState(value);
  const [focused, setFocused] = useState(false);
  useEffect(() => {
    if (!focused) setText(value);
  }, [value, focused]);
  return (
    <input
      {...props}
      value={text}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      onChange={(e) => {
        setText(e.target.value);
        onCommit(e.target.value);
      }}
    />
  );
}

