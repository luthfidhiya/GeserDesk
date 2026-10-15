// Panels over the main window are opened from places that do not own them (the menu in
// the title bar, a button in a tab), so they ask by an event on the window.

const CLIENTS = 'geserdesk:open-clients'

/** Show the clients panel. */
export const openClients = () => window.dispatchEvent(new Event(CLIENTS))

/** Call `open` whenever the clients panel is asked for; returns how to stop. */
export function onOpenClients(open: () => void) {
  window.addEventListener(CLIENTS, open)
  return () => window.removeEventListener(CLIENTS, open)
}
