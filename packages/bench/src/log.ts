import type { Clock } from '@ratchet/core';

/**
 * Bounded combined log for one bench job. Keeps the head and the tail when the cap is exceeded,
 * so the start of the run (checkout/setup) and the end (the failure) both survive.
 */
export class BenchLog {
  private head = '';
  private tail = '';
  private dropped = 0;
  private readonly headCap: number;
  private readonly tailCap: number;

  constructor(
    private readonly clock: Clock,
    capChars = 2 * 1024 * 1024,
  ) {
    this.headCap = Math.floor(capChars / 2);
    this.tailCap = Math.floor(capChars / 2);
  }

  write(text: string): void {
    if (text === '') return;
    if (this.head.length < this.headCap) {
      const room = this.headCap - this.head.length;
      this.head += text.slice(0, room);
      text = text.slice(room);
      if (text === '') return;
    }
    this.tail += text;
    if (this.tail.length > this.tailCap * 2) {
      const cut = this.tail.length - this.tailCap;
      this.dropped += cut;
      this.tail = this.tail.slice(cut);
    }
  }

  line(text: string): void {
    this.write(text.endsWith('\n') ? text : text + '\n');
  }

  /** `=== name ===` header with a timestamp. */
  section(name: string): void {
    this.line(`\n=== ${name} @ ${new Date(this.clock.now()).toISOString()} ===`);
  }

  toString(): string {
    const tail = this.tail.length > this.tailCap ? this.tail.slice(-this.tailCap) : this.tail;
    const dropped = this.dropped + (this.tail.length - tail.length);
    return dropped > 0 ? `${this.head}\n[... ${dropped} characters truncated ...]\n${tail}` : this.head + tail;
  }
}
