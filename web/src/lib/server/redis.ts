import { createClient } from 'redis'
import { getConfig } from './config'

const config = getConfig()

let redisClient: ReturnType<typeof createClient> | null = null

export async function getRedisClient() {
    if (!redisClient) {
        redisClient = createClient({
            url: config.redis.url,
        })

        redisClient.on('error', (err) => {
            console.error('Redis connection error:', err)
        })

        redisClient.on('connect', () => {
            console.log('Connected to Redis')
        })

        redisClient.on('ready', () => {
            console.log('Redis client ready')
        })

        redisClient.on('end', () => {
            console.log('Redis connection ended')
        })

        await redisClient.connect()
    }

    return redisClient
}
