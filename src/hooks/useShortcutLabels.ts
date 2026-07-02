import { useShortcutsConfig } from './useShortcutsConfig'
import { useShortcutCaptureCapability } from './useShortcutCaptureCapability'
import { useSystemShortcuts } from './useSystemShortcuts'

function formatShortcutKeys(keys: Array<{ label: string }> | undefined): string {
  return (keys ?? []).map((k) => k.label).join(' + ')
}

/**
 * Human-readable labels for the recording shortcuts, independent of how they
 * are configured: from the app's own config (macOS key capture) or from the
 * compositor's bindings (Linux/Wayland GlobalShortcuts portal).
 */
export function useShortcutLabels() {
  const { data: capability } = useShortcutCaptureCapability()
  const { data: config } = useShortcutsConfig()
  const { data: systemShortcuts } = useSystemShortcuts(capability === 'portalBind')

  if (capability === 'portalBind') {
    const trigger = (id: string) =>
      systemShortcuts?.find((s) => s.id === id)?.triggerDescription || undefined
    return {
      pushToRecordLabel: trigger('push-to-record') ?? 'your Push to Record shortcut',
      handsFreeLabel: trigger('hands-free') ?? 'your Hands-free shortcut',
    }
  }

  return {
    pushToRecordLabel: formatShortcutKeys(config?.pushToRecord.keys),
    handsFreeLabel: formatShortcutKeys(config?.handsFree.keys),
  }
}
