import { defineComponent, Fragment, h, ref, type Component, type ComponentPublicInstance } from 'vue';
import { usePluginHost } from './host';
import PluginAttachment from './components/PluginAttachment.vue';

const wrapped = new WeakSet<object>();
let nextInstance = 0;
type ComponentProps<C> = C extends abstract new (...args: never[]) => { $props: infer P } ? P : Record<string, unknown>;
export interface AttachmentComponentRef { component: ComponentPublicInstance | null }
type AttachedComponent<C> = new () => AttachmentComponentRef & { $props: ComponentProps<C> };
/** Explicit single-root ordinary-component surface. Ref exposes { component }, not a proxy of the child. */
export function withPluginAttachments<C extends Component>(component: C, options: {
  target: string;
  context: (props: ComponentProps<C>) => Record<string, unknown>;
}): AttachedComponent<C> {
  if (typeof component !== 'object' || component === null || wrapped.has(component)) throw new Error('Attachments require an unwrapped ordinary Vue component');
  const original = component as Component & { props?: Record<string, unknown>; __asyncLoader?: unknown; __isKeepAlive?: unknown; __isTeleport?: unknown; __isTransition?: unknown };
  if (original.__asyncLoader || original.__isKeepAlive || original.__isTeleport || original.__isTransition) throw new Error('Special Vue wrappers are not attachment targets');
  if (!/^[A-Za-z][A-Za-z0-9.-]{0,63}$/.test(options.target)) throw new Error('Invalid attachment target');
  const result = defineComponent({
    name: `PluginAttached${options.target}`,
    inheritAttrs: false,
    // Preserve original prop defaults/validation. No emits declaration: all listeners forward once via attrs.
    props: original.props as never,
    setup(props, { attrs, slots, expose }) {
      let contextKey = '';
      let context: Record<string, unknown> = {};
      const host = usePluginHost();
      const instanceId = `attachment-${++nextInstance}`;
      const child = ref<ComponentPublicInstance | null>(null);
      expose({ component: child });
      return () => {
        const attachments = host.attachments.value.filter(a => a.target === options.target);
        if (attachments.length) {
          const serialized = JSON.stringify(options.context(props as ComponentProps<C>));
          if (serialized !== contextKey) { contextKey = serialized; context = JSON.parse(serialized); }
        }
        const render = (position: 'before' | 'after') => attachments.filter(a => a.position === position).map(a => h(PluginAttachment, {
          key: `${a.key}/${a.generation}`, attachment: a, instanceId, context,
        }));
        return h(Fragment, null, [...render('before'), h(component, { ...props, ...attrs, ref: child }, slots), ...render('after')]);
      };
    },
  });
  wrapped.add(result);
  return result as unknown as AttachedComponent<C>;
}
