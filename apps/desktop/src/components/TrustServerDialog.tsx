import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import type { TrustPrompt } from '@/types'
import { useT } from '@/lib/i18n'

/** First contact with a server: show its fingerprint and ask. Closing = refusing. */
export function TrustServerDialog({
  prompt,
  onAnswer,
}: {
  prompt: TrustPrompt | null
  onAnswer: (trust: boolean) => void
}) {
  const t = useT()
  return (
    <Dialog open={prompt !== null} onOpenChange={(open) => !open && onAnswer(false)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('Percayai server ini?', 'Trust this server?')}</DialogTitle>
          <DialogDescription>
            {t(
              `Pertama kali menyambung ke ${prompt?.server}. Sidik jari sertifikat server:`,
              `Connecting to ${prompt?.server} for the first time. The server certificate's fingerprint:`,
            )}
          </DialogDescription>
        </DialogHeader>
        <code className="block break-all rounded-md bg-muted p-3 font-mono text-xs font-semibold">
          {prompt?.fingerprint}
        </code>
        <p className="text-xs text-muted-foreground">
          {t(
            'Cocokkan dengan sidik jari yang tampil di server (log "server certificate" atau tab Konfigurasi). Kalau berbeda, jangan dipercayai: ada yang mungkin menyamar sebagai server.',
            'Match it with the fingerprint shown on the server (the "server certificate" log line or the Configuration tab). If it differs, do not trust it: someone may be posing as the server.',
          )}
        </p>
        <DialogFooter>
          <Button variant="outline" onClick={() => onAnswer(false)}>
            {t('Batal', 'Cancel')}
          </Button>
          <Button onClick={() => onAnswer(true)}>{t('Percayai & Sambungkan', 'Trust & Connect')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
