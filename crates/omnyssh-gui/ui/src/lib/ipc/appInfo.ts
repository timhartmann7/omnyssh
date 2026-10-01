import { getVersion } from '@tauri-apps/api/app';

// The version baked into the binary: the workspace one, as `omny --version` prints it.
export const installedVersion = (): Promise<string> => getVersion();
