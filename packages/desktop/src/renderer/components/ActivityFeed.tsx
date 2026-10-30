import { AgentAvatar, Card, Eyebrow } from '@ratchet/ui';
import type { ActivityItem } from '../../shared/types';

export function ActivityFeed({ items }: { items: ActivityItem[] }) {
  return (
    <Card className="activity">
      <Eyebrow>Activity</Eyebrow>
      <ul className="activity__list">
        {items.map((a) => (
          <li key={a.id} className="activity__item">
            {a.agent ? <AgentAvatar agent={a.agent} size={22} /> : <span className={`dot dot--${a.tone}`} />}
            <span className="activity__text">{a.text}</span>
            <span className="activity__at mono">{a.at}</span>
          </li>
        ))}
      </ul>
    </Card>
  );
}
