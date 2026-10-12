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

/** First contact with a server: show its fingerprint and ask. Closing = refusing. */
export function TrustServerDialog({
  prompt,
  onAnswer,
}: {
  prompt: TrustPrompt | null
  onAnswer: (trust: boolean) => void
}) {
  return (
    <Dialog open={prompt !== null} onOpenChange={(open) => !open && onAnswer(false)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Percayai server ini?</DialogTitle>
          <DialogDescription>
            Pertama kali menyambung ke {prompt?.server}. Sidik jari sertifikat server:
          </DialogDescription>
        </DialogHeader>
        <code className="block break-all rounded-md bg-muted p-3 font-mono text-xs font-semibold">
          {prompt?.fingerprint}
        </code>
        <p className="text-xs text-muted-foreground">
          Cocokkan dengan sidik jari yang tampil di server (log &quot;server certificate&quot; atau tab
          Pengaturan monitor). Kalau berbeda, jangan dipercayai: ada yang mungkin menyamar sebagai server.
        </p>
        <DialogFooter>
          <Button variant="outline" onClick={() => onAnswer(false)}>
            Batal
          </Button>
          <Button onClick={() => onAnswer(true)}>Percayai &amp; Sambungkan</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
