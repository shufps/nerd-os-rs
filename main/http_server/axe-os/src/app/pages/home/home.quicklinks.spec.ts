import {
  DEFAULT_POOL_ICON_URL,
  getParentDomains,
  getPoolIconCandidates,
  getPoolIconUrl,
} from './home.quicklinks';

describe('home.quicklinks', () => {
  describe('getParentDomains', () => {
    it('strips one leading label down to the registrable domain', () => {
      expect(getParentDomains('nito.coinxpool.com')).toEqual(['coinxpool.com']);
      expect(getParentDomains('a.b.example.com')).toEqual(['b.example.com', 'example.com']);
    });

    it('never goes below a two-label domain', () => {
      expect(getParentDomains('coinxpool.com')).toEqual([]);
    });

    it('respects known multi-label public suffixes', () => {
      expect(getParentDomains('stratum.solohash.co.uk')).toEqual(['solohash.co.uk']);
      expect(getParentDomains('solohash.co.uk')).toEqual([]);
    });

    it('returns nothing for single-label hosts, IP literals and empty input', () => {
      expect(getParentDomains('localhost')).toEqual([]);
      expect(getParentDomains('mypool')).toEqual([]);
      expect(getParentDomains('203.0.113.7')).toEqual([]);
      expect(getParentDomains('[2001:db8::1]')).toEqual([]);
      expect(getParentDomains('2001:db8::1')).toEqual([]);
      expect(getParentDomains('')).toEqual([]);
      expect(getParentDomains(null as unknown as string)).toEqual([]);
    });

    it('normalizes case and a trailing dot', () => {
      expect(getParentDomains('NITO.CoinXPool.com.')).toEqual(['coinxpool.com']);
    });
  });

  describe('getPoolIconCandidates', () => {
    it('tries the stratum host first, then the parent domain (issue #781)', () => {
      expect(getPoolIconCandidates('stratum+tcp://nito.coinxpool.com:3333')).toEqual([
        'https://nito.coinxpool.com/favicon.ico',
        'https://coinxpool.com/favicon.ico',
      ]);
    });

    it('walks every parent level for deeper subdomains', () => {
      expect(getPoolIconCandidates('a.b.example.org:1234')).toEqual([
        'https://a.b.example.org/favicon.ico',
        'https://b.example.org/favicon.ico',
        'https://example.org/favicon.ico',
      ]);
    });

    it('has a single candidate for a registrable domain', () => {
      expect(getPoolIconCandidates('stratum+tcp://coinxpool.com:3333')).toEqual([
        'https://coinxpool.com/favicon.ico',
      ]);
    });

    it('does not strip public IP literals', () => {
      expect(getPoolIconCandidates('stratum+tcp://203.0.113.7:3333')).toEqual([
        'https://203.0.113.7/favicon.ico',
      ]);
    });

    it('uses the local default icon for local pools', () => {
      expect(getPoolIconCandidates('stratum+tcp://192.168.1.10:3333')).toEqual([DEFAULT_POOL_ICON_URL]);
      expect(getPoolIconCandidates('localhost:3333')).toEqual([DEFAULT_POOL_ICON_URL]);
      expect(getPoolIconCandidates('mypool.local:3333')).toEqual([DEFAULT_POOL_ICON_URL]);
    });

    it('keeps a registry icon override as the only candidate', () => {
      expect(getPoolIconCandidates('stratum+tcp://public-pool.io:21496')).toEqual([
        'assets/pools/public-pool.png',
      ]);
    });

    it('keeps a registry custom favicon path first and adds the parent domain', () => {
      expect(getPoolIconCandidates('stratum+tcp://solo.stratum.braiins.com:3333')).toEqual([
        'https://solo.braiins.com/icon.png',
        'https://braiins.com/favicon.ico',
      ]);
      // faviconHost is already the registrable domain (co.uk suffix): no parent
      expect(getPoolIconCandidates('stratum+tcp://eu.solohash.co.uk:3333')).toEqual([
        'https://solohash.co.uk/icons/favicon.ico',
      ]);
    });

    it('does not duplicate candidates', () => {
      // faviconHost www.mining-dutch.nl -> parent mining-dutch.nl
      expect(getPoolIconCandidates('stratum+tcp://europe.mining-dutch.nl:9999')).toEqual([
        'https://www.mining-dutch.nl/favicon.ico',
        'https://mining-dutch.nl/favicon.ico',
      ]);
    });

    it('falls back to the default icon when no host can be derived', () => {
      expect(getPoolIconCandidates('')).toEqual([DEFAULT_POOL_ICON_URL]);
    });
  });

  describe('getPoolIconUrl', () => {
    it('returns the first candidate', () => {
      expect(getPoolIconUrl('nito.coinxpool.com:3333')).toBe('https://nito.coinxpool.com/favicon.ico');
    });
  });
});
