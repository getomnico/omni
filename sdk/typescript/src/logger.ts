import pino from 'pino';

const isDev = process.env.NODE_ENV !== 'production';

const transport = isDev
  ? {
      target: 'pino-pretty',
      options: {
        colorize: true,
        levelFirst: true,
        translateTime: 'yyyy-mm-dd HH:MM:ss.l',
        ignore: 'pid,hostname',
        singleLine: true,
      },
    }
  : undefined;

const baseLogger = pino({
  level: process.env.LOG_LEVEL ?? 'info',
  ...(transport && { transport }),
});

export function getLogger(name: string): pino.Logger {
  return baseLogger.child({ module: name });
}
