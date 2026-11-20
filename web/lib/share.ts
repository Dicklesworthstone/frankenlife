// The share images (app/opengraph-image.tsx, app/twitter-image.tsx). The
// static export writes them without an extension, so worker.ts gives them
// their type; they are JPEG because a PNG of the film frame is over 2 MB,
// more than some link previewers will fetch.

export const SHARE_IMAGE_TYPE = "image/jpeg";
export const SHARE_IMAGE_PATHS: readonly string[] = ["/opengraph-image", "/twitter-image"];
