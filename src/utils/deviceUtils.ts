import type { AllDevice } from '../types/files';

export const isExternalDevice = (d: AllDevice): boolean =>
  d.hotplug || d.rm || d.tran === 'usb';

export const getDeviceIcon = (d: AllDevice): string => {
  if (d.tran === 'usb') return 'usb';
  if (d.rm) return 'sd_card';
  if (d.type === 'crypt') return 'encrypted';
  return 'hard_drive';
};

export const getDiskIcon = (d: AllDevice): string => {
  if (d.tran === 'usb') return 'usb';
  if (d.rm) return 'sd_card';
  if (d.tran === 'nvme') return 'memory';
  return 'hard_drive';
};

export const getDeviceTitle = (d: AllDevice): string => {
  const parts = [d.label || d.name];
  if (d.fstype) parts.push(d.fstype);
  if (d.size) parts.push(d.size);
  const base = parts.join(' · ');
  if (d.mounted && d.mountpoint) {
    return base + '\n' + d.devicePath + '\n→ ' + d.mountpoint;
  }
  return base + '\n' + d.devicePath;
};
