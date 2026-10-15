import { LogPanel } from '@/components/LogPanel'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'

/** The log, full size, in a window of its own. */
export default function LogWindow() {
  const { settings } = useSettings()
  const session = useSession()
  useTheme(settings?.theme)
  return (
    <div className="flex h-full min-h-0 flex-col p-4">
      <LogPanel daemonPid={session.daemon?.pid} />
    </div>
  )
}
