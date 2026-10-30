import { execSync } from 'node:child_process';

export interface Terminal {
  /** Width the output is laid out for. */
  columns: number;
  /** Number of colors the terminal advertises; below 8 only ASCII bars are drawn. */
  colors: number;
}

function shell(command: string): string {
  try {
    return execSync(command, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout: 2000 }).trim();
  } catch {
    return '';
  }
}

/** Ask the terminal database (`tput`) for the width and color count; 80 columns and 8 colors when it cannot say. */
export function detectTerminal(): Terminal {
  const columns = Number(shell('tput cols'));
  const colors = Number(shell('tput colors'));
  return {
    columns: Number.isInteger(columns) && columns > 0 ? columns : 80,
    colors: Number.isInteger(colors) && colors > 0 ? colors : 8,
  };
}
