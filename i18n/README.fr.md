<div align="center">

# ⚡ NullRouter

**Routeur d'IA et Optimiseur de Jetons Ultra Haute Performance et Zéro Copie pour le Développement Agéntique**

[![Rust](https://img.shields.io/badge/rust-%C3%A9dition%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/propuls%C3%A9%20par-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: MIT](https://img.shields.io/badge/Licence-MIT-yellow.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/latence%20de%20routage-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/fournisseurs-40%2B%20int%C3%A9gr%C3%A9s-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>Ne vous arrêtez plus jamais de coder. Économisez 20 à 40 % de tokens avec RTK, bénéficiez d'un routage sous la milliseconde et d'un basculement automatique parmi plus de 40 fournisseurs d'IA.</b>
</p>

[🚀 Démarrage Rapide](#-d%C3%A9marrage-rapide) • [✨ Fonctionnalités](#-fonctionnalit%C3%A9s-cl%C3%A9s) • [🔌 Intégrations](#-int%C3%A9gration-outils) • [🌐 Fournisseurs](#-fournisseurs-compatibles) • [📊 Performances](#-benchmarks) • [📖 Wiki](../wiki/Home.md) • [🩺 Diagnostics](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 Lire dans d'autres langues

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 Pourquoi NullRouter ?

Les assistants de programmation agéntiques modernes (Claude Code, Cursor, Cline, Roo Code, Codex) génèrent d'énormes volumes de requêtes. Les routeurs traditionnels conçus en Node.js ou Python souffrent de pauses de ramasse-miettes, d'une consommation mémoire élevée et de dizaines de millisecondes de latence superflue.

**NullRouter a été conçu intégralement en Rust autour du moteur Pingora de Cloudflare :**

| Caractéristique | NullRouter (Rust + Pingora) | Routeurs Node.js Classiques |
| :--- | :--- | :--- |
| **Moteur Proxy** | Cloudflare Pingora (HTTP asynchrone zéro-copie) | Express / Koa / Fastify |
| **Latence Interne** | **< 50 microsecondes** (voie rapide) | 15 – 45 millisecondes |
| **Empreinte Mémoire** | **~18 Mo** au repos | 120 – 350 Mo |
| **Flux SSE** | Écriture directe dans le tampon (`0` allocation intermédiaire) | Concaténations répétitives et parse JSON |
| **Compresseur RTK** | Réduction intelligente des sorties d'outils (20–40% d'économie) | Aucune compression |
| **Gestion du Raisonnement** | Claude 3.7, DeepSeek R1, o1/o3/o4 natif | Perte fréquente des blocs `<think>` |
| **Tableau de Bord** | Leptos WebAssembly SPA en 35 langues | Bundles lourds Webpack / React |

---

## ⚡ Démarrage Rapide

### 1. Lancer NullRouter

```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

Ou via le script d'exécution :
```bash
./run.sh
```

Points d'accès :
- **Point de terminaison API**: `http://localhost:20128/v1`
- **Tableau de bord**: `http://localhost:20128/dashboard`
- **Playground de Chat**: `http://localhost:20128/dashboard/chat`

---

### 2. Configuration des Outils

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Accédez à **Cursor Settings** ➔ **Models**.
2. Activez **OpenAI API Key** : `nullrouter-local`.
3. Surchargez **OpenAI Base URL** : `http://localhost:20128/v1`.
4. Ajoutez les modèles souhaités (`claude-3-7-sonnet`, `deepseek-r1`, `auto`).

---

## 📖 Documentation Wiki Complète

Consultez le [NullRouter GitHub Wiki](../wiki/Home.md) pour les guides approfondis :
- 📘 [Guide d'Installation](../wiki/Getting-Started.md)
- 🏗️ [Architecture Pingora et Zéro Copie](../wiki/Architecture-and-Performance.md)
- 💻 [Intégration d'Outils et IDEs](../wiki/Client-Integrations.md)
- 🔑 [Configuration des 40+ Fournisseurs](../wiki/Provider-Configuration.md)
- 🩺 [Guide Exhaustif de Dépannage et Diagnostics](../wiki/Diagnostics-and-Troubleshooting.md)

---

## 📄 Licence

NullRouter est un logiciel libre distribué sous licence [MIT](LICENSE).
