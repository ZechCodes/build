#!/usr/bin/env python3
"""Simple WebSocket client to test authentication with real JWT token."""

import asyncio
import json
import websockets
import requests

async def test_websocket():
    """Test WebSocket connection with real JWT token."""
    
    # Test both real JWT token and demo token
    print("Testing with real JWT token first...")
    try:
        response = requests.get("http://localhost:8000/demo/token")
        if response.status_code == 200:
            token_data = response.json()
            token = token_data["access_token"]
            print(f"Got demo token: {token[:50]}...")
        else:
            print(f"Failed to get token: {response.status_code}")
            token = "demo-token"
    except Exception as e:
        print(f"Error getting token: {e}")
        token = "demo-token"
    
    print(f"Will test with demo-token fallback as well...")
    
    # Test WebSocket connection with the token
    uri = f"ws://localhost:8000/ws/terminal?token={token}"
    print(f"Connecting to: {uri}")
    
    try:
        async with websockets.connect(uri) as websocket:
            print("WebSocket connected successfully!")
            
            # Send a test message
            test_message = {
                "type": "join_session",
                "session_id": "test-session-123"
            }
            await websocket.send(json.dumps(test_message))
            print(f"Sent: {test_message}")
            
            # Wait for response
            try:
                response = await asyncio.wait_for(websocket.recv(), timeout=5.0)
                print(f"Received: {response}")
            except asyncio.TimeoutError:
                print("No response received within 5 seconds")
            
    except websockets.exceptions.ConnectionClosedError as e:
        print(f"WebSocket connection closed: {e}")
    except Exception as e:
        print(f"WebSocket connection failed: {e}")
    
    # Now test with demo-token
    print("\n" + "="*50)
    print("Testing with demo-token...")
    demo_uri = "ws://localhost:8000/ws/terminal?token=demo-token"
    print(f"Connecting to: {demo_uri}")
    
    try:
        async with websockets.connect(demo_uri) as websocket:
            print("WebSocket connected successfully with demo-token!")
            
            # Send a test message
            test_message = {
                "type": "join_session",
                "session_id": "test-session-456"
            }
            await websocket.send(json.dumps(test_message))
            print(f"Sent: {test_message}")
            
            # Wait for response
            try:
                response = await asyncio.wait_for(websocket.recv(), timeout=5.0)
                print(f"Received: {response}")
            except asyncio.TimeoutError:
                print("No response received within 5 seconds")
            
    except websockets.exceptions.ConnectionClosedError as e:
        print(f"WebSocket connection closed with demo-token: {e}")
    except Exception as e:
        print(f"WebSocket connection failed with demo-token: {e}")

if __name__ == "__main__":
    asyncio.run(test_websocket())