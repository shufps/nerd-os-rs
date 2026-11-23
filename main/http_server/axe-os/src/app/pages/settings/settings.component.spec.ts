import { discardPeriodicTasks, fakeAsync, flush, tick } from '@angular/core/testing';
import { Observable, of, throwError } from 'rxjs';
import { IUpdateStatus } from 'src/app/models/IUpdateStatus';
import { GithubUpdateService } from '../../services/github-update.service';
import { githubOtaOutcome, isProgressStep } from './github-ota-status';
import { SettingsComponent } from './settings.component';

describe('githubOtaOutcome', () => {
  const st = (step: string, running = true, pending = false): IUpdateStatus =>
    ({ pending, running, step, progress: 0 });

  it('keeps polling while the update is pending or running', () => {
    expect(githubOtaOutcome(st('idle', false, true))).toBe('running');
    expect(githubOtaOutcome(st('resolving_url'))).toBe('running');
    expect(githubOtaOutcome(st('updating_fw'))).toBe('running');
    expect(githubOtaOutcome(null)).toBe('running');
  });

  it('reports success on rebooting', () => {
    expect(githubOtaOutcome(st('rebooting'))).toBe('rebooting');
    expect(githubOtaOutcome(st('rebooting', false))).toBe('rebooting');
  });

  it('reports the error step as failed, running or not', () => {
    expect(githubOtaOutcome(st('error', false))).toBe('failed');
    expect(githubOtaOutcome(st('error', true))).toBe('failed');
  });

  it('reports a task that ended without rebooting as failed', () => {
    // e.g. the device restarted into the old firmware after a failed flash write
    expect(githubOtaOutcome(st('idle', false, false))).toBe('failed');
    expect(githubOtaOutcome(st('resolving_url', false, false))).toBe('failed');
  });

  it('isProgressStep', () => {
    expect(isProgressStep('resolving_url')).toBeTrue();
    expect(isProgressStep('idle')).toBeFalse();
    expect(isProgressStep('error')).toBeFalse();
    expect(isProgressStep('')).toBeFalse();
  });
});

describe('SettingsComponent GitHub update', () => {
  let statuses: Array<IUpdateStatus | 'http-error'>;
  let system: any;
  let toastr: jasmine.SpyObj<any>;
  let c: SettingsComponent;

  const translate: any = {
    instant: (key: string, params?: any) => (params ? `${key}${JSON.stringify(params)}` : key),
  };

  function nextStatus(): Observable<IUpdateStatus> {
    const s = statuses.length > 1 ? statuses.shift()! : statuses[0];
    return s === 'http-error' ? throwError(() => new Error('offline')) : of(s);
  }

  beforeEach(() => {
    statuses = [];
    system = {
      getSettingsV2: () => of({}),
      performGithubOTAUpdate: jasmine.createSpy('performGithubOTAUpdate').and.returnValue(of({ status: 'started' })),
      getGithubOTAStatus: jasmine.createSpy('getGithubOTAStatus').and.callFake(() => nextStatus()),
      getInfo: jasmine.createSpy('getInfo').and.returnValue(throwError(() => new Error('offline'))),
    };
    toastr = jasmine.createSpyObj('toastr', ['success', 'danger', 'warning']);
    const otp: any = { ensureOtp$: () => of({ totp: undefined }) };

    c = new SettingsComponent(system, toastr, {} as any, new GithubUpdateService({} as any), translate, otp);
    c.expectedFactoryFilename = 'factory.bin';
    c.selectedRelease = {
      id: 1, tag_name: 'v1.0.0', name: 'v1.0.0', prerelease: false, body: '', published_at: '',
      assets: [{ id: 1, name: 'factory.bin', browser_download_url: 'https://github.com/x/factory.bin', size: 1 }],
    };
  });

  afterEach(() => c.ngOnDestroy());

  it('stops the spinner and shows the error on resolving_url -> error (issue #779)', fakeAsync(() => {
    statuses = [
      { pending: true, running: false, step: 'idle', progress: 0 },
      { pending: false, running: true, step: 'resolving_url', progress: 0 },
      { pending: false, running: false, step: 'error', progress: 0 },
    ];
    c.directUpdateFromGithub();
    expect(c.isOneClickUpdate).toBeTrue();

    tick(1000);
    tick(1000);
    expect(c.isOneClickUpdate).toBeTrue();
    tick(1000);

    expect(c.isOneClickUpdate).toBeFalse();
    expect(c.otaError).toBe('UPDATE.GITHUB_ERROR_STEP{"step":"UPDATE.STEP_RESOLVING_URL"}');
    expect(toastr.danger).toHaveBeenCalledTimes(1);
    expect(toastr.danger.calls.mostRecent().args[0]).toContain('TOAST.UPDATE_FAILED');
    expect(toastr.danger.calls.mostRecent().args[0]).toContain('UPDATE.STEP_RESOLVING_URL');

    // polling stopped
    const calls = system.getGithubOTAStatus.calls.count();
    tick(5000);
    expect(system.getGithubOTAStatus.calls.count()).toBe(calls);
  }));

  it('shows a generic error when the first status is already error', fakeAsync(() => {
    statuses = [{ pending: false, running: false, step: 'error', progress: 0 }];
    c.directUpdateFromGithub();
    tick(1000);
    expect(c.isOneClickUpdate).toBeFalse();
    expect(c.otaError).toBe('UPDATE.GITHUB_ERROR');
    expect(toastr.danger).toHaveBeenCalledTimes(1);
  }));

  it('keeps polling through request errors and reports a restart into the old firmware', fakeAsync(() => {
    statuses = [
      { pending: false, running: true, step: 'updating_fw', progress: 10 },
      'http-error',
      'http-error',
      { pending: false, running: false, step: 'idle', progress: 0 },
    ];
    c.directUpdateFromGithub();
    tick(1000);
    tick(1000);
    tick(1000);
    expect(c.isOneClickUpdate).toBeTrue();
    expect(toastr.danger).not.toHaveBeenCalled();
    tick(1000);
    expect(c.isOneClickUpdate).toBeFalse();
    expect(c.otaError).toContain('UPDATE.STEP_UPDATING_FW');
  }));

  it('clears a previous error on a new attempt', fakeAsync(() => {
    statuses = [{ pending: false, running: false, step: 'error', progress: 0 }];
    c.directUpdateFromGithub();
    tick(1000);
    expect(c.otaError).not.toBe('');

    statuses = [{ pending: false, running: true, step: 'resolving_url', progress: 0 }];
    c.directUpdateFromGithub();
    expect(c.otaError).toBe('');
    expect(c.isOneClickUpdate).toBeTrue();
    tick(1000);
    expect(c.isOneClickUpdate).toBeTrue();
    c.ngOnDestroy();
    discardPeriodicTasks();
  }));

  it('still reports success on rebooting', fakeAsync(() => {
    statuses = [
      { pending: false, running: true, step: 'updating_www', progress: 90 },
      { pending: false, running: true, step: 'rebooting', progress: 100 },
    ];
    c.directUpdateFromGithub();
    tick(2000);
    expect(toastr.success).toHaveBeenCalledTimes(1);
    expect(toastr.danger).not.toHaveBeenCalled();
    expect(c.isOneClickUpdate).toBeTrue();
    c.ngOnDestroy();
    flush();
    discardPeriodicTasks();
  }));

  it('stops the spinner when the start request fails', fakeAsync(() => {
    system.performGithubOTAUpdate.and.returnValue(throwError(() => ({ message: 'busy' })));
    c.directUpdateFromGithub();
    expect(c.isOneClickUpdate).toBeFalse();
    expect(toastr.danger).toHaveBeenCalledTimes(1);
  }));
});
