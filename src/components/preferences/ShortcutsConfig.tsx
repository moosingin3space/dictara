import { KeyCaptureInput } from '@/components/shortcuts/KeyCaptureInput'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { useShortcutsConfig } from '@/hooks/useShortcutsConfig'
import { useSaveShortcutsConfig } from '@/hooks/useSaveShortcutsConfig'
import { useResetShortcutsConfig } from '@/hooks/useResetShortcutsConfig'
import { useShortcutCaptureCapability } from '@/hooks/useShortcutCaptureCapability'
import { useSystemShortcuts } from '@/hooks/useSystemShortcuts'
import { useConfigureSystemShortcuts } from '@/hooks/useConfigureSystemShortcuts'
import { SettingsIcon } from 'lucide-react'

export function ShortcutsConfiguration() {
  const { data: capability } = useShortcutCaptureCapability()

  if (capability === 'portalBind') {
    return <PortalShortcutsConfiguration />
  }
  if (capability === 'unavailable') {
    return (
      <Alert>
        <AlertDescription>
          Global keyboard shortcuts are not available in this session. Recording can still be
          started from the tray menu.
        </AlertDescription>
      </Alert>
    )
  }
  return <CaptureShortcutsConfiguration />
}

/**
 * Linux/Wayland: shortcut bindings are owned by the compositor via the
 * GlobalShortcuts portal. We can only display the current triggers and ask
 * the system to open its configuration dialog — in-app key capture never
 * sees raw key events on Wayland.
 */
function PortalShortcutsConfiguration() {
  const { data: shortcuts } = useSystemShortcuts(true)
  const configureMutation = useConfigureSystemShortcuts()

  return (
    <div className="space-y-6">
      {shortcuts?.map((shortcut) => (
        <div key={shortcut.id} className="space-y-3">
          <div>
            <h3 className="text-base font-medium">
              {shortcut.id === 'push-to-record' ? 'Push to Record' : 'Hands-free'}
            </h3>
            <p className="text-sm text-muted-foreground mt-0.5">{shortcut.description}</p>
          </div>
          <div className="flex items-center gap-2 flex-wrap h-[58px] p-3 border-2 rounded-lg">
            {shortcut.triggerDescription ? (
              <div className="flex items-center gap-1.5 px-3 py-1.5 bg-background border rounded-md text-sm font-medium">
                {shortcut.triggerDescription}
              </div>
            ) : (
              <span className="text-sm text-muted-foreground">
                No trigger assigned — use the system dialog below
              </span>
            )}
          </div>
        </div>
      ))}

      <Alert>
        <AlertDescription>
          Shortcuts are managed by your desktop environment. Note: unlike macOS, the trigger keys
          also reach the focused application, so prefer combinations you don&apos;t use for typing.
        </AlertDescription>
      </Alert>

      {configureMutation.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            {configureMutation.error?.message || 'Failed to open the system shortcut dialog'}
          </AlertDescription>
        </Alert>
      )}

      <div className="flex justify-end pt-4">
        <Button
          onClick={() => configureMutation.mutate()}
          variant="outline"
          disabled={configureMutation.isPending}
        >
          <SettingsIcon className="h-4 w-4 mr-2" />
          Change in System Dialog
        </Button>
      </div>
    </div>
  )
}

/** macOS: in-app "press your keys" capture via the CGEvent tap. */
function CaptureShortcutsConfiguration() {
  const { data: config } = useShortcutsConfig()
  const saveMutation = useSaveShortcutsConfig()
  const resetMutation = useResetShortcutsConfig()

  if (!config) {
    return <div>Loading...</div>
  }

  const handlePushToRecordChange = (keys: Array<{ keycode: number; label: string }>) => {
    saveMutation.mutate({
      ...config,
      pushToRecord: { keys },
    })
  }

  const handleHandsFreeChange = (keys: Array<{ keycode: number; label: string }>) => {
    saveMutation.mutate({
      ...config,
      handsFree: { keys },
    })
  }

  const handleReset = () => {
    resetMutation.mutate()
  }

  return (
    <div className="space-y-6">
      <KeyCaptureInput
        label="Push to Record"
        description="Hold to record, release to stop"
        value={config.pushToRecord.keys}
        onChange={handlePushToRecordChange}
      />

      <KeyCaptureInput
        label="Hands-free"
        description="Press to toggle (start/stop)"
        value={config.handsFree.keys}
        onChange={handleHandsFreeChange}
      />

      {saveMutation.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            {saveMutation.error?.message || 'Failed to save shortcuts'}
          </AlertDescription>
        </Alert>
      )}

      <div className="flex justify-end pt-4">
        <Button onClick={handleReset} variant="outline" disabled={resetMutation.isPending}>
          Reset to Defaults
        </Button>
      </div>
    </div>
  )
}
