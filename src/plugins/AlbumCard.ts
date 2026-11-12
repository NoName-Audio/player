import AlbumCard from '../components/AlbumCard.vue';
import { withPluginAttachments } from './withPluginAttachments';

/**
 * Album card shown in the artist album list. Attachments render as siblings
 * before or after the card, occupying their own flex/grid item; keep UI compact.
 * Context exposes the current album through `album`, not the component instance.
 * This explicit registration and its inferred context type are the source for
 * the generated attachment target reference (`pnpm attachments:docs`).
 */
export default withPluginAttachments(AlbumCard, {
  target: 'AlbumCard',
  context: props => ({ album: props.album }),
});
