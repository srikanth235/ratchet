/**
 * Time zones: which names are valid and what they are (standard offset, daylight-saving offset). The table of
 * every zone the runtime knows is built when the module loads, by asking Intl for the UTC offset in force on
 * the 15th of every second month of a reference year.
 */
export interface ZoneInfo {
  name: string;
  /** Standard UTC offset in minutes. */
  standard: number;
  /** Daylight-saving UTC offset in minutes, or null when the zone has none. */
  daylight: number | null;
}

const REFERENCE_YEAR = 2026;
const SAMPLES = Array.from({ length: 6 }, (_, i) => Date.UTC(REFERENCE_YEAR, i * 2, 15, 12));

function parseOffset(label: string): number {
  const m = /GMT(?:([+-])(\d{2}):(\d{2}))?/.exec(label);
  if (!m || !m[1]) return 0;
  return (m[1] === '-' ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3]));
}

function describeZone(name: string): ZoneInfo {
  const format = new Intl.DateTimeFormat('en-US', { timeZone: name, timeZoneName: 'longOffset', hour: 'numeric' });
  const offsets = SAMPLES.map((ms) => parseOffset(format.formatToParts(ms).find((p) => p.type === 'timeZoneName')?.value ?? 'GMT'));
  const standard = Math.min(...offsets);
  const peak = Math.max(...offsets);
  return { name, standard, daylight: peak > standard ? peak : null };
}

const ZONES = new Map<string, ZoneInfo>();
for (const name of ['UTC', ...Intl.supportedValuesOf('timeZone')]) ZONES.set(name, describeZone(name));

/** Aliases such as Asia/Kolkata are not in the canonical list but the runtime resolves them. */
function resolves(name: string): boolean {
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: name });
    return true;
  } catch {
    return false;
  }
}

export function isKnownZone(name: string): boolean {
  return ZONES.has(name) || resolves(name);
}

export function zoneInfo(name: string): ZoneInfo {
  const zone = ZONES.get(name) ?? (resolves(name) ? describeZone(name) : undefined);
  if (!zone) throw new RangeError(`unknown time zone ${JSON.stringify(name)}`);
  return zone;
}

export function formatOffset(minutes: number): string {
  const sign = minutes < 0 ? '-' : '+';
  const abs = Math.abs(minutes);
  return `UTC${sign}${String(Math.floor(abs / 60)).padStart(2, '0')}:${String(abs % 60).padStart(2, '0')}`;
}

const dayFormats = new Map<string, Intl.DateTimeFormat>();

/** The calendar date (YYYY-MM-DD) of an instant in a time zone. */
export function localDate(ms: number, zone: string): string {
  let f = dayFormats.get(zone);
  if (!f) {
    f = new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit' });
    dayFormats.set(zone, f);
  }
  return f.format(ms);
}
