// Joins a buffered snapshot (`terminal.read` / `process.read`) with live
// stream events without losing or duplicating output.
//
// Live events are queued until the snapshot arrives; afterwards only events
// starting at or after the snapshot's `next` offset are written. Offsets are
// UTF-8 byte offsets, as produced by the runtime.

const encoder = new TextEncoder();

export function utf8Length(text: string): number {
  return encoder.encode(text).length;
}

export class OutputSync {
  private readonly write: (data: string) => void;
  private queue: Array<{ offset: number; data: string; rendered: string }> = [];
  private next: number | null = null;

  constructor(write: (data: string) => void) {
    this.write = write;
  }

  /** Offset expected for the next chunk, or null before the snapshot. */
  get position(): number | null {
    return this.next;
  }

  /**
   * A live chunk from a stream event. `rendered` is what gets written (e.g.
   * the data wrapped in color codes); offsets always refer to `data`.
   */
  push(offset: number, data: string, rendered: string = data): void {
    if (this.next === null) {
      this.queue.push({ offset, data, rendered });
      return;
    }
    if (offset < this.next) return; // already written (part of the snapshot)
    this.write(rendered);
    this.next = offset + utf8Length(data);
  }

  /** The buffered output, `next` being the offset right after it. */
  snapshot(data: string, next: number): void {
    if (data) this.write(data);
    this.next = next;
    const queued = this.queue;
    this.queue = [];
    for (const chunk of queued) this.push(chunk.offset, chunk.data, chunk.rendered);
  }
}
