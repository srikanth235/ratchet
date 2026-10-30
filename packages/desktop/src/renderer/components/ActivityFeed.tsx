import { useState } from 'react';
import { Card, agentLabel } from '@ratchet/ui';
import type { ActivityItem } from '../../shared/types';
import { useQuery } from '../data';

const GLYPH: Record<ActivityItem['tone'], string> = { mint: '✓', rose: '×', brass: '◆', neutral: '–' };

/** `Verified win: Short-circuit … · 138 → 131 ms` → title `Verified win`, text the rest. */
function splitText(text: string): { title: string; body: string } {
  const i = text.indexOf(': ');
  return i > 0 && i < 40 ? { title: text.slice(0, i), body: text.slice(i + 2) } : { title: '', body: text };
}

export function ActivityFeed({ items }: { items: ActivityItem[] }) {
  const [all, setAll] = useState(false);
  const more = useQuery(() => window.ratchet.recentActivity({ limit: 30 }), [], { skip: !all, intervalMs: 10_000 });
  const shown = all && more.data ? more.data : items;
  return (
    <Card className="activity" data-testid="activity">
      <div className="activity__head">
        <h2 className="card-title">Activity</h2>
        <button type="button" className="text-link" onClick={() => setAll((v) => !v)}>{all ? 'Show less' : 'View all'}</button>
      </div>
      {shown.length === 0 ? (
        <p className="muted">Nothing yet.</p>
      ) : (
        <ul className="activity__list">
          {shown.map((a) => {
            const { title, body } = splitText(a.text);
            return (
              <li key={a.id} className="activity__item">
                <span className={`activity__glyph activity__glyph--${a.tone}`} aria-hidden="true">{GLYPH[a.tone]}</span>
                <div className="activity__main">
                  <div className="activity__top">
                    <span className="activity__title">{title || (a.agent ? agentLabel(a.agent) : 'Update')}{title && a.agent ? ` · ${agentLabel(a.agent)}` : ''}</span>
                    <span className="activity__at mono">{a.at}</span>
                  </div>
                  <span className="activity__text">{body}</span>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}
