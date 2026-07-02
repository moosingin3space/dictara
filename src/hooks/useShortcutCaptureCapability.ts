import { useQuery } from '@tanstack/react-query'
import { commands, type ShortcutCaptureCapability } from '@/bindings'

/**
 * Hook to detect how shortcuts can be configured on this platform:
 * - `rawCapture`: in-app "press your keys" capture (macOS)
 * - `portalBind`: compositor-owned bindings via the GlobalShortcuts portal
 *   (Linux/Wayland) — configuration happens in the system dialog
 * - `unavailable`: no global shortcut support in this session
 */
export function useShortcutCaptureCapability() {
  return useQuery({
    queryKey: ['shortcutCaptureCapability'],
    queryFn: async (): Promise<ShortcutCaptureCapability> => {
      return await commands.getShortcutCaptureCapability()
    },
    // Platform capability never changes while the app is running
    staleTime: Infinity,
  })
}
