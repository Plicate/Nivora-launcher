// минимальные типы qrcode без зависимости от @types/node
declare module 'qrcode' {
  interface QrStringOptions {
    type?: 'svg' | 'utf8' | 'terminal'
    errorCorrectionLevel?: 'L' | 'M' | 'Q' | 'H'
    margin?: number
    width?: number
    color?: { dark?: string; light?: string }
  }
  const QRCode: {
    toString(text: string, options?: QrStringOptions): Promise<string>
  }
  export default QRCode
}
