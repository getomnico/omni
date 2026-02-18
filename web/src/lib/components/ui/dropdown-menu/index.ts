import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui'
import Content from './dropdown-menu-content.svelte'
import Item from './dropdown-menu-item.svelte'
import Separator from './dropdown-menu-separator.svelte'

const Root = DropdownMenuPrimitive.Root
const Trigger = DropdownMenuPrimitive.Trigger
const Portal = DropdownMenuPrimitive.Portal

export {
    Root,
    Trigger,
    Portal,
    Content,
    Item,
    Separator,
    //
    Root as DropdownMenu,
    Trigger as DropdownMenuTrigger,
    Portal as DropdownMenuPortal,
    Content as DropdownMenuContent,
    Item as DropdownMenuItem,
    Separator as DropdownMenuSeparator,
}
