import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '../store/store';

// Returns DLC folder chosen in setting
export async function getActiveDlcFolder(): Promise<string | null> {
  return (
    useAppStore.getState().dlcFolder ||
    (await invoke<string | null>('get_dlc_folder', {}))
  );
}

/**
 * Validates DLC folder chosen.
 * @returns `DLC` folder path, or null if not civ install.
 */
export async function resolveDlcFolder(path: string): Promise<string | null> {
  return await invoke<string | null>('resolve_dlc_folder', { path });
}
