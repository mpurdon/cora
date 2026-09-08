import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ipc } from "../../lib/ipc";

/** The repo whose PR is on screen. Private attachments can only be signed in
 *  the context of a repo the viewer can read, and the markdown renderer is
 *  too far from the detail panel to be handed one as a prop. */
export const AttachmentRepo = createContext("");

/** A pasted screenshot on a private repo: `github.com/user-attachments/assets/<id>`
 *  or the older `github.com/<owner>/<repo>/assets/<user>/<id>`. Those serve a
 *  sign-in page to a bare `<img>`; the backend fetches them through GitHub's
 *  markdown renderer instead. Public images pass straight through. */
function isPrivateAttachment(src: string): boolean {
  if (!src.startsWith("https://github.com/")) return false;
  if (!src.includes("/assets/")) return false;
  return /\/[0-9a-f-]{36}$/i.test(src.split("?")[0] ?? "");
}

/** One fetch per attachment for the life of the window; the backend keeps
 *  the bytes on disk across launches. */
const resolved = new Map<string, Promise<string>>();

/** The image's displayable URL: itself, or the signed bytes for a private
 *  attachment. `null` while that is in flight. */
function useImageUrl(src: string): { url: string | null; failed: boolean } {
  const repo = useContext(AttachmentRepo);
  const needsSigning = isPrivateAttachment(src) && repo !== "";
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!needsSigning) return;
    let live = true;
    const key = `${repo} ${src}`;
    let p = resolved.get(key);
    if (!p) {
      p = ipc.githubAttachment(repo, src);
      resolved.set(key, p);
      p.catch(() => resolved.delete(key));
    }
    p.then((u) => live && setDataUrl(u)).catch(() => live && setFailed(true));
    return () => {
      live = false;
    };
  }, [needsSigning, repo, src]);
  return { url: needsSigning ? dataUrl : src, failed };
}

// ---- Gallery -------------------------------------------------------------

/** Every image in one rendered body, in order, so a click on any of them
 *  opens a lightbox that pages through all of them. */
const Gallery = createContext<{ images: string[]; open: (src: string) => void } | null>(null);

const MD_IMAGE = /!\[[^\]]*\]\(\s*<?([^\s)>]+)>?[^)]*\)|<img\b[^>]*\ssrc=["']([^"']+)["']/g;

/** Badges (shields, CI status) are decoration, not screenshots — they stay
 *  inline and out of the gallery. */
function isBadge(src: string): boolean {
  const path = src.split("?")[0] ?? "";
  return /shields\.io|badge|\.svg$/i.test(path);
}

/** Image sources in a markdown body, unique and in order of appearance. */
function imageSources(markdown: string): string[] {
  const out: string[] = [];
  for (const m of markdown.matchAll(MD_IMAGE)) {
    const src = (m[1] ?? m[2] ?? "").trim();
    if (src && !isBadge(src) && !out.includes(src)) out.push(src);
  }
  return out;
}

/** Wrap a rendered markdown body: its images become gallery thumbnails. */
export function ImageGallery({ body, children }: { body: string; children: ReactNode }) {
  const images = useMemo(() => imageSources(body), [body]);
  const [current, setCurrent] = useState<string | null>(null);
  const ctx = useMemo(() => ({ images, open: setCurrent }), [images]);
  return (
    <Gallery.Provider value={ctx}>
      {children}
      {current !== null && (
        <Lightbox
          images={images.includes(current) ? images : [current]}
          current={current}
          onCurrent={setCurrent}
          onClose={() => setCurrent(null)}
        />
      )}
    </Gallery.Provider>
  );
}

function Lightbox({
  images,
  current,
  onCurrent,
  onClose,
}: {
  images: string[];
  current: string;
  onCurrent: (src: string) => void;
  onClose: () => void;
}) {
  const index = Math.max(0, images.indexOf(current));
  const step = (d: number) => onCurrent(images[(index + d + images.length) % images.length]!);
  useEffect(() => {
    // Capture phase so the rail's j/k and other global keys don't fire
    // while the lightbox has the screen.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      else if (e.key === "ArrowRight" || e.key === "ArrowDown" || e.key === " ") step(1);
      else if (e.key === "ArrowLeft" || e.key === "ArrowUp") step(-1);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });
  return createPortal(
    <div className="lightbox" onClick={onClose} role="dialog" aria-label="Image viewer">
      <button className="lightbox-close" onClick={onClose} aria-label="Close">
        ✕
      </button>
      <div className="lightbox-stage">
        {images.length > 1 && (
          <button
            className="lightbox-nav prev"
            aria-label="Previous image"
            onClick={(e) => {
              e.stopPropagation();
              step(-1);
            }}
          >
            ‹
          </button>
        )}
        <div className="lightbox-frame" onClick={(e) => e.stopPropagation()}>
          <ImageOrPending key={current} src={current} alt="" className="lightbox-img" />
        </div>
        {images.length > 1 && (
          <button
            className="lightbox-nav next"
            aria-label="Next image"
            onClick={(e) => {
              e.stopPropagation();
              step(1);
            }}
          >
            ›
          </button>
        )}
      </div>
      {images.length > 1 && (
        <div className="lightbox-strip" onClick={(e) => e.stopPropagation()}>
          {images.map((src, i) => (
            <button
              key={src}
              className={`lightbox-thumb${i === index ? " current" : ""}`}
              onClick={() => onCurrent(src)}
              aria-label={`Image ${i + 1} of ${images.length}`}
              aria-current={i === index}
            >
              <ImageOrPending src={src} alt="" className="lightbox-thumb-img" />
            </button>
          ))}
        </div>
      )}
      <div className="lightbox-count">
        {index + 1} / {images.length}
      </div>
    </div>,
    document.body,
  );
}

/** The image once its URL is known, a placeholder until then. */
function ImageOrPending({ src, alt, className }: { src: string; alt: string; className?: string }) {
  const { url, failed } = useImageUrl(src);
  if (!url) {
    return (
      <span
        className={`${className ?? ""} md-img-pending${failed ? " failed" : ""}`}
        title={failed ? "Couldn't load this attachment" : "Loading…"}
      >
        {alt || (failed ? "image unavailable" : "loading image…")}
      </span>
    );
  }
  return <img src={url} alt={alt} className={className} loading="lazy" />;
}

/** An image in rendered markdown: a gallery thumbnail when the body has a
 *  gallery and it's a real image, a plain inline image otherwise. */
export function AttachedImage({ src, alt, className }: { src: string; alt: string; className?: string }) {
  const gallery = useContext(Gallery);
  const inGallery = gallery !== null && gallery.images.includes(src);
  if (!inGallery) return <ImageOrPending src={src} alt={alt} className={className} />;
  return (
    <button
      type="button"
      className="md-thumb"
      onClick={() => gallery.open(src)}
      title={alt || "Open image"}
      aria-label={alt || "Open image"}
    >
      <ImageOrPending src={src} alt={alt} className={className} />
    </button>
  );
}
