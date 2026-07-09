# appMirror — POC de déport d'affichage mono-fenêtre vers navigateur

Streaming basse latence d'**une seule fenêtre native** vers une page web, avec
injection des événements souris/clavier retour vers la fenêtre ciblée.

```
┌─────────────┐   JPEG frames (WS binaire)   ┌──────────────────┐
│  Navigateur │  <-------------------------   │  Backend Rust    │
│  (canvas)   │                               │                  │
│             │   events souris/clavier (WS   │  WGC capture ───┐│
│             │   JSON, coords natives) ───▶  │  JPEG encode    ││
└─────────────┘                               │  SendInput  ◀───┘│
                                              └──────────────────┘
```

## Choix techniques (POC)

| Domaine        | Choix POC                              | Upgrade production                          |
|----------------|----------------------------------------|---------------------------------------------|
| Transport      | WebSocket (binaire pour la vidéo)      | WebRTC (`webrtc-rs`) — DataChannel + track  |
| Encodage       | JPEG (crate `image`, qualité réglable) | H.264 via Media Foundation / `openh264`     |
| Capture Win    | Windows Graphics Capture (`windows-capture`) | idem, + surfaces D3D11 sans copie CPU |
| Capture Linux  | *non implémenté* (voir `capture/`)     | PipeWire (portail) ou X11 `XShmGetImage`    |
| Injection Win  | `SendInput` (coords absolues écran)    | injection ciblée sur HWND en arrière-plan   |

> Le POC vise Windows 11. La couche capture/injection est isolée derrière des
> modules `#[cfg(windows)]` pour permettre une implémentation Linux ultérieure.

## Prérequis

- Rust stable (>= 1.75) — `rustup`
- Node.js >= 18 (pour builder le frontend)
- Windows 10 2004+ / Windows 11 (WGC)

## Démarrage

```bash
# 1. Frontend
cd frontend
npm install
npm run dev        # dev server Vite sur http://localhost:5173 (proxy /ws -> :8080)
# ou pour un build statique servi par le backend :
npm run build      # génère frontend/dist

# 2. Backend
cd ../backend
cargo run          # écoute sur http://127.0.0.1:8080 (ws sur /ws)
```

En dev : ouvrez http://localhost:5173. En build statique : http://127.0.0.1:8080.

## Latence & sécurité — notes POC

- `SendInput` agit sur la **fenêtre au premier plan** : le backend appelle
  `SetForegroundWindow` sur la cible à la sélection. Pour une injection en
  arrière-plan (sans voler le focus) il faut passer par `PostMessage` — mais ça
  ne marche pas avec les apps DirectX/jeux. Voir `input/windows.rs`.
- Le serveur n'a **aucune authentification** : ne l'exposez pas hors localhost
  tel quel. Ajoutez un token/TLS avant tout usage réseau.
- Les versions exactes des API de `windows-capture` évoluent : si `cargo build`
  échoue sur `Settings::new` ou `frame.buffer()`, alignez avec la version
  installée (`cargo doc -p windows-capture --open`).
