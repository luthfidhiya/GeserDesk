import { LayoutEditor } from '@/components/LayoutEditor'
import type { DeskSnapshot } from '@/types'

interface Props {
  desk: DeskSnapshot | null
  running: boolean
  refresh: () => Promise<void>
  onBusy: (busy: boolean) => void
}

export function LayoutView({ desk, running, refresh, onBusy }: Props) {
  if (desk) return <LayoutEditor desk={desk} refresh={refresh} onBusy={onBusy} />
  return (
    <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
      {running
        ? 'Menunggu server siap…'
        : 'Jalankan server (tab Konfigurasi) untuk mengatur monitor dan menyetujui komputer.'}
    </div>
  )
}
