import { describe, expect, it } from "vitest";
import {
  baseName,
  detectEol,
  formatBytes,
  formatDuration,
  isInside,
  joinPath,
  parentPath,
  stripAnsi,
  toLf,
  withEol,
} from "./format";

describe("formatBytes", () => {
  it("formats units", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(20 * 1024 * 1024)).toBe("20 MB");
    expect(formatBytes(-1)).toBe("—");
  });
});

describe("formatDuration", () => {
  it("formats ms, seconds and minutes", () => {
    expect(formatDuration(42)).toBe("42 ms");
    expect(formatDuration(1500)).toBe("1.50 s");
    expect(formatDuration(95_000)).toBe("1 min 35 s");
  });
});

describe("paths", () => {
  it("joins with the path's own separator", () => {
    expect(joinPath("/home/ana", "src")).toBe("/home/ana/src");
    expect(joinPath("C:\\Projetos", "MeuSaaS")).toBe("C:\\Projetos\\MeuSaaS");
    expect(joinPath("/", "etc")).toBe("/etc");
    expect(joinPath("C:\\", "Users")).toBe("C:\\Users");
  });

  it("computes base names", () => {
    expect(baseName("/home/ana/file.ts")).toBe("file.ts");
    expect(baseName("C:\\Projetos\\MeuSaaS\\")).toBe("MeuSaaS");
    expect(baseName("/")).toBe("");
  });

  it("computes parents and stops at roots", () => {
    expect(parentPath("/home/ana")).toBe("/home");
    expect(parentPath("/home")).toBe("/");
    expect(parentPath("/")).toBe("/");
    expect(parentPath("C:\\Projetos\\MeuSaaS")).toBe("C:\\Projetos");
    expect(parentPath("C:\\Projetos")).toBe("C:\\");
    expect(parentPath("C:\\")).toBe("C:\\");
  });

  it("checks containment across separators", () => {
    expect(isInside("/a/b/c", "/a/b")).toBe(true);
    expect(isInside("/a/bc", "/a/b")).toBe(false);
    expect(isInside("C:\\a\\b", "C:\\a")).toBe(true);
  });
});

describe("stripAnsi", () => {
  it("removes colors and OSC sequences", () => {
    expect(stripAnsi("\u001b[32mok\u001b[0m done")).toBe("ok done");
    expect(stripAnsi("\u001b]0;title\u0007text")).toBe("text");
  });
});

describe("line endings", () => {
  it("round-trips CRLF files through LF editing", () => {
    const original = "a\r\nb\r\n";
    const eol = detectEol(original);
    expect(eol).toBe("\r\n");
    const edited = `${toLf(original)}c\n`;
    expect(withEol(edited, eol)).toBe("a\r\nb\r\nc\r\n");
  });

  it("keeps LF files untouched", () => {
    expect(detectEol("a\nb")).toBe("\n");
    expect(withEol("a\nb", "\n")).toBe("a\nb");
  });
});
