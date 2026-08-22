<script lang="ts">
  // A `<select>` with the platform's control chrome stripped off. Left native, WebKit
  // paints macOS's own popup button — a light grey gradient with a double arrow — and
  // ignores every colour utility on it, so the field sat on the dark form as a bright
  // hole that matched none of the inputs beside it. GTK and Windows draw their own
  // variations of the same problem. The chevron below is ours, so the closed control
  // is identical on every OS; the open list stays the platform's, which is the point
  // of using a real `<select>` (keyboard, screen readers, touch all keep working).
  import type { Snippet } from 'svelte';

  let {
    value = $bindable(),
    class: className = '',
    children
  }: { value: string; class?: string; children: Snippet } = $props();
</script>

<span class="relative block">
  <!-- `pr-9` keeps the longest option clear of the chevron. -->
  <select bind:value class="{className} appearance-none pr-9">
    {@render children()}
  </select>
  <svg
    class="pointer-events-none absolute right-3 top-1/2 h-3 w-3 -translate-y-1/2 text-faint"
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M3 4.5 6 7.5 9 4.5" />
  </svg>
</span>
