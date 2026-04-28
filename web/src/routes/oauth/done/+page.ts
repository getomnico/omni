import type { PageLoad } from './$types'

export const load: PageLoad = ({ url }) => {
    return {
        ok: url.searchParams.get('ok') === 'true',
        sourceId: url.searchParams.get('sourceId'),
        message: url.searchParams.get('message'),
    }
}
