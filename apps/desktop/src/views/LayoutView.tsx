import { LayoutEditor } from '@/components/LayoutEditor'
import { useT } from '@/lib/i18n'
import type { DeskSnapshot } from '@/types'

interface Props {
  desk: DeskSnapshot | null
  running: boolean
  refresh: () => Promise<void>
  onBusy: (busy: boolean) => void
}

export function LayoutView({ desk, running, refresh, onBusy }: Props) {
  const t = useT()
  if (desk) return <LayoutEditor desk={desk} refresh={refresh} onBusy={onBusy} />
  return (
    <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
      {running
        ? t('Menunggu server siap…', 'Waiting for the server to be ready…')
        : t('Jalankan server (tab Konfigurasi) untuk mengatur monitor dan menyetujui komputer.', 'Start the server (Configuration tab) to arrange the monitors and approve computers.')}
    </div>
  )
}
