import { useMutation, useQueryClient } from '@tanstack/react-query'
import { commands } from '@/bindings'

/**
 * Hook to open the compositor's shortcut configuration dialog
 * (Linux/Wayland GlobalShortcuts portal). Invalidates the system shortcuts
 * list afterwards so updated trigger descriptions are refetched.
 */
export function useConfigureSystemShortcuts() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (): Promise<void> => {
      const result = await commands.configureSystemShortcuts()
      if (result.status === 'error') {
        throw new Error(result.error)
      }
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['systemShortcuts'] })
    },
  })
}
