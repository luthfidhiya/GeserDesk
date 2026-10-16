import type { Lang } from '@/lib/i18n'

// The messages that reach the window from the Rust side come from crates the CLI shares,
// so they are written once, some in Indonesian and some in English. The ones the window
// shows often are put into the window's language here; anything else is shown as it is.

type Words = (...groups: string[]) => string
const KNOWN: [RegExp, Words, Words][] = [
  // From the app and the daemon.
  [/Sudah berjalan\. Hentikan dulu\./, () => 'Sudah berjalan. Hentikan dulu.', () => 'Already running. Stop it first.'],
  [/tidak ada daemon yang berjalan/, () => 'tidak ada daemon yang berjalan', () => 'no daemon is running'],
  [/tidak bisa menjalankan daemon: (.*)/, (e) => `tidak bisa menjalankan daemon: ${e}`, (e) => `could not start the daemon: ${e}`],
  [/daemon tidak siap: (.*) \(lihat daemon\.log\)/, (e) => `daemon tidak siap: ${e} (lihat daemon.log)`, (e) => `the daemon is not ready: ${e} (see daemon.log)`],
  [/daemon tidak berjalan \((.*)\)/, (e) => `daemon tidak berjalan (${e})`, (e) => `the daemon is not running (${e})`],
  [/daemon tidak menjawab/, () => 'daemon tidak menjawab', () => 'the daemon does not answer'],
  [/daemon terputus/, () => 'daemon terputus', () => 'the daemon went away'],
  [/daemon menutup koneksi/, () => 'daemon menutup koneksi', () => 'the daemon closed the connection'],
  [/server belum berjalan/, () => 'server belum berjalan', () => 'the server is not running yet'],
  [/Isi alamat server dulu \(contoh (.*)\)\./, (e) => `Isi alamat server dulu (contoh ${e}).`, (e) => `Fill in the server address first (e.g. ${e}).`],
  [/Isi nama layar komputer ini\./, () => 'Isi nama layar komputer ini.', () => "Fill in this computer's screen name."],
  [/tidak bisa menyuntik input: (.*)/, (e) => `tidak bisa menyuntik input: ${e}`, (e) => `could not inject input: ${e}`],
  [/Masih ada perintah yang berjalan\. Hentikan dulu\./, () => 'Masih ada perintah yang berjalan. Hentikan dulu.', () => 'A command is still running. Stop it first.'],
  [/tanda kutip belum ditutup/, () => 'tanda kutip belum ditutup', () => 'a quote is not closed'],
  // From the server's desk.
  [/layar tidak dikenal: (.*)/, (e) => `layar tidak dikenal: ${e}`, (e) => `unknown screen: ${e}`],
  [/layar (.*) dan (.*) saling tumpang tindih/, (a, b) => `layar ${a} dan ${b} saling tumpang tindih`, (a, b) => `screens ${a} and ${b} overlap`],
  [/TLS mati di server ini/, () => 'TLS mati di server ini', () => 'TLS is off on this server'],
  [/sidik jari tidak valid/, () => 'sidik jari tidak valid', () => 'invalid fingerprint'],
  [/tidak ada permintaan itu yang menunggu \(mungkin sudah kedaluwarsa; biarkan client mencoba lagi\)/, () => 'tidak ada permintaan itu yang menunggu (mungkin sudah kedaluwarsa; biarkan client mencoba lagi)', () => 'that request is no longer waiting (it may have expired; let the client try again)'],
  [/layar itu tidak punya sertifikat yang dipercayai/, () => 'layar itu tidak punya sertifikat yang dipercayai', () => 'that screen has no trusted certificate'],
  [/layar sendiri tidak bisa dinonaktifkan/, () => 'layar sendiri tidak bisa dinonaktifkan', () => "the server's own screen cannot be turned off"],
  [/gagal menyimpan config: (.*)/, (e) => `gagal menyimpan config: ${e}`, (e) => `could not save the config: ${e}`],
  [/port harus antara 1 dan 65535/, () => 'port harus antara 1 dan 65535', () => 'the port must be between 1 and 65535'],
  [/jump zone minimal 1 px/, () => 'zona tepi minimal 1 px', () => 'the edge zone must be at least 1 px'],
  [/ukuran sudut tidak boleh negatif/, () => 'ukuran sudut tidak boleh negatif', () => 'the corner size cannot be negative'],
  // From keys.toml.
  [/the combination is used twice/, () => 'kombinasi dipakai dua kali', () => 'the combination is used twice'],
  [/the action is set twice/, () => 'aksi diatur dua kali', () => 'the action is set twice'],
  [/unknown action "([^"]*)"/, (a) => `aksi "${a}" tidak dikenal`, (a) => `unknown action "${a}"`],
  [/unknown action$/, () => 'aksi tidak dikenal', () => 'unknown action'],
  [/unknown key "([^"]*)"/, (k) => `tombol "${k}" tidak dikenal`, (k) => `unknown key "${k}"`],
  [/"([^"]*)" has two keys/, (c) => `"${c}" berisi dua tombol`, (c) => `"${c}" has two keys`],
  [/"([^"]*)" has no key, only modifiers/, (c) => `"${c}" hanya berisi tombol pengubah, tanpa tombol`, (c) => `"${c}" has no key, only modifiers`],
  [/"([^"]*)" is not a number/, (v) => `"${v}" bukan angka`, (v) => `"${v}" is not a number`],
  [/"([^"]*)" is not a virtual key \(1 to 0xFE\)/, (v) => `"${v}" bukan virtual key (1 sampai 0xFE)`, (v) => `"${v}" is not a virtual key (1 to 0xFE)`],
  [/: use ignore, keysym:0x\.\.\., vk:0x\.\.\. or action:name/, () => ': pakai ignore, keysym:0x..., vk:0x... atau action:nama', () => ': use ignore, keysym:0x..., vk:0x... or action:name'],
  [/: not a received copy/, () => ': bukan salinan yang diterima', () => ': not a received copy'],
  [/no config folder on this machine/, () => 'tidak ada folder konfigurasi di komputer ini', () => 'no config folder on this machine'],
]

/** `text` with the known messages in it put into `lang`. */
export function localizeError(text: string, lang: Lang): string {
  let out = text
  for (const [re, id, en] of KNOWN) {
    out = out.replace(re, (_, ...rest) => {
      const groups = rest.slice(0, -2).map(String)
      return (lang === 'en' ? en : id)(...groups)
    })
  }
  return out
}
