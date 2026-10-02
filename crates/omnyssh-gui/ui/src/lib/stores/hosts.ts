import { writable } from 'svelte/store';
import type { HostDto } from '$lib/bindings';

/** The known hosts, kept in sync by the `hosts-loaded` event (tech-gui.md §3.5). */
export const hosts = writable<HostDto[]>([]);

/** The first host of each name. The backend opens a name on its first host, so a
 *  later namesake can't be reached by name and a picker lists it once. */
export function firstOfEachName(list: HostDto[]): HostDto[] {
  const seen = new Set<string>();
  return list.filter((h) => {
    if (seen.has(h.name)) return false;
    seen.add(h.name);
    return true;
  });
}
