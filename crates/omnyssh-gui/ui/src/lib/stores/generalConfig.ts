import { get, writable } from "svelte/store";
import type { GeneralConfigDto } from "$lib/bindings";
import { loadGeneralConfig } from "$lib/ipc/commands";

const config = writable<GeneralConfigDto | null>(null);

let loading: Promise<GeneralConfigDto> | null = null;

export const generalConfigStore = {
  subscribe: config.subscribe,

  get value(): GeneralConfigDto | null {
    return get(config);
  },

  async load(): Promise<GeneralConfigDto> {
    const current = get(config);

    if (current) {
      return current;
    }

    if (loading) {
      return loading;
    }

    loading = loadGeneralConfig();

    try {
      const next = await loading;
      config.set(next);
      return next;
    } finally {
      loading = null;
    }
  },

  set(next: GeneralConfigDto): void {
    config.set(next);
  },
};
