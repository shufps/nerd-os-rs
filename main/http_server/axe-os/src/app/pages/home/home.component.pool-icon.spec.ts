import { HomeComponent } from './home.component';
import { DEFAULT_EXTERNAL_POOL_ICON_URL, DEFAULT_POOL_ICON_URL } from './home.quicklinks';

/**
 * Pool icon fallback walk (issue #781). Only the icon helpers are exercised,
 * so the component is created without DI and the icon state set up by hand.
 */
describe('HomeComponent pool icon fallback', () => {
  let c: HomeComponent;

  function errorOn(img: HTMLImageElement, host: string) {
    c.onPoolIconError({ target: img } as unknown as Event, host);
  }

  function imgWith(src: string): HTMLImageElement {
    const img = document.createElement('img');
    img.src = src;
    return img;
  }

  beforeEach(() => {
    c = Object.create(HomeComponent.prototype) as HomeComponent;
    const any = c as any;
    any.poolIconErrorCache = new Set<string>();
    any.poolIconCandidates = new Map<string, string[]>();
    any.poolIconIndex = new Map<string, number>();
  });

  it('steps from the stratum host favicon to the parent domain, then the default', () => {
    const host = 'nito.coinxpool.com';
    expect(c.poolIconUrl(host)).toBe('https://nito.coinxpool.com/favicon.ico');

    const img = imgWith(c.poolIconUrl(host));
    errorOn(img, host);
    expect(img.src).toBe('https://coinxpool.com/favicon.ico');
    expect(c.poolIconUrl(host)).toBe('https://coinxpool.com/favicon.ico');

    errorOn(img, host);
    expect(img.src).toContain(DEFAULT_EXTERNAL_POOL_ICON_URL);
    expect(c.poolIconUrl(host)).toBe(DEFAULT_EXTERNAL_POOL_ICON_URL);

    // a failing default icon does not loop
    errorOn(img, host);
    expect(img.src).toContain(DEFAULT_EXTERNAL_POOL_ICON_URL);
  });

  it('does not skip a candidate when two images share a host', () => {
    const host = 'a.b.example.org';
    const img1 = imgWith(c.poolIconUrl(host));
    const img2 = imgWith(c.poolIconUrl(host));

    errorOn(img1, host);
    errorOn(img2, host); // late error for the same (first) candidate
    expect(img1.src).toBe('https://b.example.org/favicon.ico');
    expect(img2.src).toBe('https://b.example.org/favicon.ico');
    expect(c.poolIconUrl(host)).toBe('https://b.example.org/favicon.ico');
  });

  it('goes straight to the default when the host has no parent domain', () => {
    const host = 'coinxpool.com';
    const img = imgWith(c.poolIconUrl(host));
    errorOn(img, host);
    expect(img.src).toContain(DEFAULT_EXTERNAL_POOL_ICON_URL);
  });

  it('uses the local default icon for local pools', () => {
    expect(c.poolIconUrl('192.168.1.10')).toBe(DEFAULT_POOL_ICON_URL);
    expect(c.poolIconUrl('')).toBe(DEFAULT_POOL_ICON_URL);
  });
});
