import { describe, expect, it } from 'vitest';
import { findLabel, HIGHLIGHT_LIMIT, seedQuery } from './terminalFind';

describe('findLabel — the find bar count', () => {
  it('says nothing before a query is typed', () => {
    expect(findLabel({ kind: 'idle' })).toBe('');
  });

  it('says so when the regex does not compile', () => {
    expect(findLabel({ kind: 'invalid' })).toBe('Invalid pattern');
  });

  it('says when nothing matches', () => {
    expect(findLabel({ kind: 'matches', index: -1, count: 0 })).toBe('No results');
  });

  it('counts from one', () => {
    expect(findLabel({ kind: 'matches', index: 0, count: 3 })).toBe('1 of 3');
    expect(findLabel({ kind: 'matches', index: 2, count: 3 })).toBe('3 of 3');
  });

  it('marks a count that reached the highlight limit as a lower bound', () => {
    expect(findLabel({ kind: 'matches', index: 4, count: HIGHLIGHT_LIMIT })).toBe(
      `5 of ${HIGHLIGHT_LIMIT}+`
    );
    expect(findLabel({ kind: 'matches', index: -1, count: HIGHLIGHT_LIMIT })).toBe(
      `${HIGHLIGHT_LIMIT}+ matches`
    );
  });
});

describe('seedQuery — a selection pre-fills the find bar', () => {
  it('takes a one-line selection as is', () => {
    expect(seedQuery('error: connection refused')).toBe('error: connection refused');
  });

  it('ignores a selection across lines', () => {
    expect(seedQuery('first\nsecond')).toBe('');
    expect(seedQuery('first\r\nsecond')).toBe('');
  });
});
