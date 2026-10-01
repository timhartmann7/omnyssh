// Single bootstrap subscribing every backend event into the stores
// (tech-gui.md §3.5). Returns a disposer that drops all listeners; if any
// listen fails, already-registered listeners are disposed so none leak.

import type { UnlistenFn } from '@tauri-apps/api/event';
import { events } from '$lib/bindings';
import {
  applyError,
  applyFilePreview,
  applyHostStatusChanged,
  applyHostsLoaded,
  applyKeySetupComplete,
  applyKeySetupFailed,
  applyKeySetupProgress,
  applyKeySetupRollback,
  applyMetricsUpdated,
  applyServicesDetected,
  applyServicesFailed,
  applyUpdateAvailable,
  applySftpConnected,
  applySftpDirListed,
  applySftpDisconnected,
  applySftpOpDone,
  applySnippetResult,
  applyTerminalExited,
  applyTransferProgress,
  applyTunnelStatusChanged,
  applyKeyPassphraseRequired,
  applyPasswordRequired
} from './router';

export async function startEventBridge(): Promise<() => void> {
  const offs: UnlistenFn[] = [];
  try {
    offs.push(await events.hostsLoaded.listen((e) => applyHostsLoaded(e.payload)));
    offs.push(await events.hostStatusChanged.listen((e) => applyHostStatusChanged(e.payload)));
    offs.push(await events.metricsUpdated.listen((e) => applyMetricsUpdated(e.payload)));
    offs.push(await events.servicesDetected.listen((e) => applyServicesDetected(e.payload)));
    offs.push(await events.servicesFailed.listen((e) => applyServicesFailed(e.payload)));
    offs.push(await events.tunnelStatusChanged.listen((e) => applyTunnelStatusChanged(e.payload)));
    offs.push(await events.snippetResult.listen((e) => applySnippetResult(e.payload)));
    offs.push(
      await events.terminalExited.listen((e) =>
        applyTerminalExited(e.payload.sessionId, e.payload.hadOutput)
      )
    );
    offs.push(await events.sftpConnected.listen((e) => applySftpConnected(e.payload)));
    offs.push(await events.sftpDirListed.listen((e) => applySftpDirListed(e.payload)));
    offs.push(await events.sftpOpDone.listen((e) => applySftpOpDone(e.payload)));
    offs.push(await events.sftpDisconnected.listen((e) => applySftpDisconnected(e.payload)));
    offs.push(await events.filePreview.listen((e) => applyFilePreview(e.payload)));
    offs.push(await events.transferProgress.listen((e) => applyTransferProgress(e.payload)));
    offs.push(await events.keySetupProgress.listen((e) => applyKeySetupProgress(e.payload)));
    offs.push(await events.keySetupComplete.listen((e) => applyKeySetupComplete(e.payload)));
    offs.push(await events.keySetupFailed.listen((e) => applyKeySetupFailed(e.payload)));
    offs.push(await events.keySetupRollback.listen((e) => applyKeySetupRollback(e.payload)));
    offs.push(await events.updateAvailable.listen((e) => applyUpdateAvailable(e.payload)));
    offs.push(await events.keyPassphraseRequired.listen((e) => applyKeyPassphraseRequired(e.payload)));
    offs.push(await events.passwordRequired.listen((e) => applyPasswordRequired(e.payload)));
    offs.push(await events.error.listen((e) => applyError(e.payload.message)));
  } catch (err) {
    offs.forEach((off) => off());
    throw err;
  }
  return () => offs.forEach((off) => off());
}
