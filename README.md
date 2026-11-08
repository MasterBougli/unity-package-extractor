# Unity Package Extractor

Outil Windows en **Rust** pour extraire le contenu d’un fichier `.unitypackage` (FBX, textures, matériaux, prefabs, etc.) avec :

- interface graphique (sans fenêtre console)
- file d’attente d’extraction
- nom de dossier de sortie personnalisable
- extraction **séquentielle ou multithread**
- serveur **MCP** pour Cursor / agents IA

**Auteur :** Bougli  
**Licence :** [CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/) (non commercial · attribution · share-alike)

---

## Avertissement important (assets tiers)

Ce dépôt contient **uniquement** le code de l’extracteur.

Il **n’inclut pas** :

- de packages Unity / Synty
- de FBX, textures, matériaux ou autres assets
- de contenu soumis aux licences Unity Asset Store / éditeurs tiers

Vous êtes **seul responsable** de vérifier et de respecter les licences des fichiers `.unitypackage` que vous extrayez.  
L’auteur (Bougli) n’est pas affilié à Unity Technologies ni à Synty Studios.

---

## Fonctionnalités

| Fonction | Description |
|---|---|
| Extraction fidèle | Recrée l’arborescence et les noms de fichiers d’origine via `pathname` |
| File d’attente | Empile plusieurs packages, édite les noms de dossiers, extrait en série ou en parallèle |
| Logs | Console de logs d’extraction en direct |
| MCP | Outils `list_packages`, `extract_package`, `extract_queue` |
| Options | Inclure / exclure `.meta` et aperçus `preview.png` |

---

## Binaires

| Exécutable | Rôle |
|---|---|
| `unity_extractor.exe` | Interface graphique (1024×600) |
| `unity_extractor_mcp.exe` | Serveur MCP (stdio) |
| `batch_extract.exe` | Extraction CLI en lot |

Compilation :

```powershell
cargo build --release
```

Les binaires se trouvent dans `target/release/`.

---

## Interface graphique

1. Glissez-déposez un ou plusieurs `.unitypackage` (ou parcourez)
2. Choisissez le **dossier parent** et le **nom du dossier** de sortie
3. Ajoutez à la **file d’attente**
4. Activez éventuellement **Multithread / parallel extract**
5. Cliquez sur **Extract queue**
6. Suivez la progression et les **logs**

---

## Serveur MCP (Cursor)

Exemple de configuration `~/.cursor/mcp.json` :

```json
{
  "mcpServers": {
    "unity-extractor": {
      "command": "C:\\chemin\\vers\\unity_extractor_mcp.exe",
      "args": []
    }
  }
}
```

### Outils

- `list_packages` — liste les `.unitypackage` d’un dossier
- `extract_package` — extrait un package (`folder_name` optionnel)
- `extract_queue` — extrait une file d’attente

---

## Format `.unitypackage`

Un `.unitypackage` est une archive `tar.gz`. Chaque asset est stocké sous un GUID ; le fichier `pathname` indique le chemin réel dans le projet Unity. L’outil reconstitue cette structure à l’extraction.

---

## Licence

Copyright © 2026 **Bougli**

Distribué sous **Creative Commons Attribution-NonCommercial-ShareAlike 4.0 International**  
([résumé](https://creativecommons.org/licenses/by-nc-sa/4.0/) · [texte légal](LICENSE))

En résumé :

- **Attribution** obligatoire (citer Bougli)
- **Non commercial** — pas d’usage commercial sans autorisation
- **ShareAlike** — les versions modifiées redistribuées doivent rester sous la même licence
- Une **concession de brevets** limitée (usage non commercial) est décrite dans `LICENSE`

Pour un usage commercial, contactez l’auteur.

---

## Contribution

Les contributions sont les bienvenues pour un usage non commercial, sous la même licence CC BY-NC-SA 4.0.
