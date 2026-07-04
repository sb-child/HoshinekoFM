import { useCallback } from 'react';
import { showToast } from '../utils/toast';
import { t } from '../i18n';
import { FileSystemService } from '../services/FileSystemService';

export function useDeviceActions() {
  const handleDeviceMount = useCallback(async (devicePath: string) => {
    showToast(t('device.mounting'), 'info');
    const result = await FileSystemService.mountDevice(devicePath);
    if (result.success) {
      showToast(t('device.mounted'), 'success');
    } else {
      showToast(`${t('device.mount_failed')}: ${result.error || ''}`, 'error');
    }
    return result;
  }, []);

  const handleDeviceUnmount = useCallback(async (devicePath: string) => {
    showToast(t('device.unmounting'), 'info');
    const result = await FileSystemService.unmountDevice(devicePath);
    if (result.success) {
      showToast(t('device.unmounted'), 'success');
    } else {
      showToast(`${t('device.unmount_failed')}: ${result.error || ''}`, 'error');
    }
  }, []);

  const handleDeviceEject = useCallback(async (devicePath: string) => {
    showToast(t('device.unmounting'), 'info');
    const result = await FileSystemService.ejectDevice(devicePath);
    if (result.success) {
      showToast(t('device.unmounted'), 'success');
    } else {
      showToast(`${t('device.eject_failed')}: ${result.error || ''}`, 'error');
    }
  }, []);

  return { handleDeviceMount, handleDeviceUnmount, handleDeviceEject };
}
