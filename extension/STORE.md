# Edge Add-ons listing

What to paste into Microsoft Partner Center (partner.microsoft.com/dashboard/microsoftedge).
Package: zip the *contents* of this folder (manifest.json at the zip's top),
leaving out README.md and STORE.md: `tools/package-extension.sh`.

## Availability
- Visibility: **Hidden** (only people with the link can find and install it).
- Markets: all.
- Category: Productivity.

## Properties
- Privacy policy URL: https://wall.poncatoera.workers.dev/extension-privacy.html
- Website: https://wall.poncatoera.workers.dev
- Mature content: No.

## Listing (English)

**Name**: Wall

**Short description** (≤ 132 characters):
Right-click any picture, video or page to add it to a Wall board, in the Wall app or Wall on the web.

**Description**:
Collect references while you browse. Right-click any picture, video or link and choose Add to Wall, and it lands on the board you pick: the board open in the Wall app, any other board in your library, or Wall on the web in a tab.

- Pictures come in at full size, even ones you only see signed in.
- Videos from YouTube, Vimeo, Instagram, X and more are downloaded by the Wall app. Mark a start and end while the video plays to keep just that clip.
- Screenshot the whole page (Alt+Shift+X) or drag out an area (Alt+Shift+A).
- Everything remembers the page it came from.

Needs the Wall app on the same computer (or Wall on the web open in a tab). What you add goes only to your own Wall.

**Search terms**: moodboard, reference, inspiration, collect, screenshot, video clip, designers

## Permission notes for reviewers
- `<all_urls>` host access: fetch the right-clicked picture with the page's sign-in, take screenshots, read the video's current time for clip marks, and hand files to an open Wall page (content script `webwall.js`, only on wall.poncatoera.workers.dev and localhost).
- `cookies`: only for vimeo.com, instagram.com, x.com, twitter.com, tiktok.com, facebook.com, threads.net and patreon.com, only when the user adds a video or link from one of them; passed to the Wall app at 127.0.0.1, which uses them only to sign in to that same site for the download, then deletes them.
- `scripting`: area-screenshot overlay and reading the video's time, on the user's click.
- No remote code. Network use: downloads the picture the user chose from its own website, and talks to the Wall app at 127.0.0.1:47830; files sent to Wall on the web are saved by that page through the Wall server.

## Testing notes for reviewers
The extension works with the Wall desktop app (Windows). Without it, open https://wall.poncatoera.workers.dev in a tab, then right-click a picture on any page > Add to Wall > Wall on the web.

## Partner Center "Single purpose" and permission justification fields
- Single purpose: Wall collects visual references from the web into the user's Wall boards. The user right-clicks a picture, video, link or page, or takes a screenshot, and the extension sends that item to the Wall desktop app on the same computer or to a Wall web page the user has open.
- contextMenus: the "Add to Wall" right-click item on pictures, videos, links and pages, listing the boards to send to.
- storage: the board last chosen and clip start/end marks, in extension storage only.
- alarms: refresh the menu's board list once a minute while Wall is open (MV3 menus can't update as they open).
- scripting: on the user's request only: area-screenshot overlay, reading a video's current time, fetching a picture the page loaded itself (blob: URLs).
- cookies: as in "Permission notes" above.
- Remote code: none. Data handled (disclose, don't tick "none"): site cookies for the listed video sites, page addresses and titles, and the content the user picks; all go only to the user's own Wall (the app at 127.0.0.1, or their open Wall page, which saves it through the Wall server like any drop); cookies are used only to sign in to that same site for the download.

# Chrome Web Store listing

Same zip as Edge (`tools/package-extension.sh`). Dashboard: chrome.google.com/webstore/devconsole.
Account: one-time US$5 registration fee, 2-Step Verification on the Google account, a verified contact email,
and the EU trader question: **non-trader** (personal tool, not selling it).

## Package tab
Upload `wall-extension-<version>.zip`. Icon comes from the zip (icons/128.png).

## Store listing tab
- Description: the Edge **Description** above (the first line doubles as the summary; the manifest description is the short one).
- Category: Productivity (Workflow & Planning). Language: English.
- Screenshots: store-screenshot-1.png (1280x800).
- Small promo tile (required): store-tile-440x280.png. Marquee (optional): store-tile-1400x560.png.
- Homepage URL: https://wall.poncatoera.workers.dev

## Privacy tab
- Single purpose: as the Edge "Single purpose" above.
- Permission justifications: contextMenus, storage, alarms, scripting, cookies as above.
  Host permission (`<all_urls>`): fetch the picture the user right-clicked with that page's sign-in, take the screenshot the user asked for, read a video's current time for clip marks, and hand items to the user's open Wall page.
- Remote code: **No, I am not using remote code.**
- Data collected: tick **Authentication information** (site cookies, video sites only, passed to the user's own Wall app), **Website content** (the picture, video or screenshot the user picks) and **Web history** (the address and title of the page an item came from). Nothing else.
- Tick all three certifications (not sold, not used for unrelated purposes, not used for credit).
- Privacy policy URL: https://wall.poncatoera.workers.dev/extension-privacy

## Distribution tab
- Visibility: **Unlisted** (only people with the link). Regions: all. Free.

## Test instructions (Chrome has a field for these)
As the Edge "Testing notes for reviewers" above. No sign-in needed.
