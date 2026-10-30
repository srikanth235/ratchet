import { useEffect, useRef, useState } from 'react';
import type { AgentEventVM } from '../../shared/types';
import { errMsg } from '../data';
import { fmtClock } from '../format';

const MAX_LINES = 300;

/**
 * Streams an agent's event log: the newest page first, then only events after the cursor,
 * fetched whenever the engine reports activity for this agent (plus a slow poll as a safety net).
 */
export function LiveLog({ agentId, pollMs = 3000 }: { agentId: string; pollMs?: number }) {
  const [lines, setLines] = useState<AgentEventVM[]>([]);
  const [error, setError] = useState<string | null>(null);
  const cursor = useRef<number | undefined>(undefined);
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    let alive = true;
    cursor.current = undefined;
    setLines([]);
    const pull = () => {
      window.ratchet.agentEvents(agentId, cursor.current).then(
        (page) => {
          if (!alive) return;
          setError(null);
          if (page.events.length === 0) return;
          cursor.current = page.cursor;
          setLines((prev) => [...prev, ...page.events].slice(-MAX_LINES));
        },
        (e: unknown) => alive && setError(errMsg(e)),
      );
    };
    pull();
    const off = window.ratchet.onEvent((e) => {
      if (e.type === 'agent-event' && e.agentId === agentId) pull();
    });
    const timer = setInterval(pull, pollMs);
    return () => {
      alive = false;
      off();
      clearInterval(timer);
    };
  }, [agentId, pollMs]);

  useEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  return (
    <div
      ref={box}
      className="log mono"
      role="log"
      aria-label="Agent log"
      aria-live="polite"
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      }}
    >
      {error && <div className="log__line log__line--error">{error}</div>}
      {lines.length === 0 && !error && <div className="log__line muted">No events yet.</div>}
      {lines.map((l) => (
        <div key={l.id} className={`log__line log__line--${l.type}`}>
          <span className="log__at">{fmtClock(l.at)}</span>
          <span className="log__type">{l.type}</span>
          <span className="log__text">{l.text}</span>
        </div>
      ))}
    </div>
  );
}
