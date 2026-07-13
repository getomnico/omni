import { Drawer as DrawerPrimitive } from "vaul-svelte";
import Content from "./drawer-content.svelte";

const Root = DrawerPrimitive.Root;
const Trigger = DrawerPrimitive.Trigger;
const Portal = DrawerPrimitive.Portal;
const Overlay = DrawerPrimitive.Overlay;
const Close = DrawerPrimitive.Close;
const Title = DrawerPrimitive.Title;
const Description = DrawerPrimitive.Description;

export {
	Root,
	Close,
	Trigger,
	Portal,
	Overlay,
	Content,
	Title,
	Description,
	//
	Root as Drawer,
	Close as DrawerClose,
	Trigger as DrawerTrigger,
	Portal as DrawerPortal,
	Overlay as DrawerOverlay,
	Content as DrawerContent,
	Title as DrawerTitle,
	Description as DrawerDescription,
};
