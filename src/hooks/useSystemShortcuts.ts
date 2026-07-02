import { useQuery } from '@tanstack/react-query'
import { commands, type SystemShortcut } from '@/bindings'

/**
 * Hook to list shortcuts as currently bound by the compositor
 * (Linux/Wayland GlobalShortcuts portal). Only enable when the capture
 * capability is `portalBind`.
 */
export function useSystemShortcuts(enabled: boolean) {
  return useQuery({
    queryKey: ['systemShortcuts'],
    queryFn: async (): Promise<SystemShortcut[]> => {
      const result = await commands.listSystemShortcuts()
      if (result.status === 'error') {
        throw new Error(result.error)
      }
      return result.data
    },
    enabled,
  })
}
