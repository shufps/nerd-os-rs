// Mirrors Board::Error enum order from board.h
export const BOARD_ERROR_STRINGS: Record<number, string> = {
  0: '',
  1: 'MINER OVERHEATED',
  2: 'VREG OVERHEATED',
  3: 'PSU FAULT',
  4: 'CURRENT PROTECTION',
  5: 'VOLTAGE PROTECTION',
};

export function boardErrorStr(code: number | undefined): string {
  if (!code) return '';
  return BOARD_ERROR_STRINGS[code] ?? 'ERROR';
}

/** The hardware protection errors (1..5) as text; '' for none or another error. */
export function hardwareBoardError(code: number | undefined): string {
  return code && code >= 1 && code <= 5 ? boardErrorStr(code) : '';
}
